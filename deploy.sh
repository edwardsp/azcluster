#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<EOF
Usage: $0 --name NAME --location LOCATION [options]

Required:
  --name NAME                Cluster name (2-20 chars, used for resource naming)
  --location LOCATION        Azure region (e.g. uksouth, eastus2)

Options:
  --resource-group NAME      Use existing RG instead of creating rg-azcluster-<name>
  --ssh-key PATH             SSH public key file (default: \$HOME/.ssh/id_rsa.pub or id_ed25519.pub)
  --login-public-ip          Give login VM a public IP (default: off)
  --allowed-ssh-cidrs CSV    Restrict SSH to these CIDRs (comma-separated)
  --azcluster-version TAG    GitHub release tag (default: v0.0.1)
  --azcluster-repo OWNER/REPO  GitHub repo (default: edwardsp/azcluster)
  --ubuntu 2204|2404         Ubuntu HPC image SKU (default: 2404)
  --anf-size-tib N           ANF capacity pool size in TiB (default: 2)
  --anf-tier TIER            ANF service level: Standard|Premium|Ultra (default: Standard)
  --compute-pool NAME        Compute pool / Slurm partition name (default: gpu)
  --compute-sku SKU          Compute VM SKU (default: Standard_ND96isr_H100_v5)
  --compute-count N          Initial VMSS Flex capacity (default: 0)
  --what-if                  Dry-run only; show what would change
  -h, --help                 Show this help

Example:
  $0 --name demo --location southafricanorth --login-public-ip --compute-count 2
EOF
}

CLUSTER_NAME=""
LOCATION=""
EXISTING_RG=""
SSH_KEY=""
LOGIN_PUBLIC_IP="false"
ALLOWED_SSH_CIDRS=""
AZCLUSTER_VERSION="v0.1.0"
AZCLUSTER_REPO="edwardsp/azcluster"
UBUNTU_SKU="2404"
ANF_SIZE_TIB="2"
ANF_TIER="Standard"
COMPUTE_POOL="gpu"
COMPUTE_SKU="Standard_ND96isr_H100_v5"
COMPUTE_COUNT="0"
WHAT_IF="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --name) CLUSTER_NAME="$2"; shift 2 ;;
    --location) LOCATION="$2"; shift 2 ;;
    --resource-group) EXISTING_RG="$2"; shift 2 ;;
    --ssh-key) SSH_KEY="$2"; shift 2 ;;
    --login-public-ip) LOGIN_PUBLIC_IP="true"; shift ;;
    --allowed-ssh-cidrs) ALLOWED_SSH_CIDRS="$2"; shift 2 ;;
    --azcluster-version) AZCLUSTER_VERSION="$2"; shift 2 ;;
    --azcluster-repo) AZCLUSTER_REPO="$2"; shift 2 ;;
    --ubuntu) UBUNTU_SKU="$2"; shift 2 ;;
    --anf-size-tib) ANF_SIZE_TIB="$2"; shift 2 ;;
    --anf-tier) ANF_TIER="$2"; shift 2 ;;
    --compute-pool) COMPUTE_POOL="$2"; shift 2 ;;
    --compute-sku) COMPUTE_SKU="$2"; shift 2 ;;
    --compute-count) COMPUTE_COUNT="$2"; shift 2 ;;
    --what-if) WHAT_IF="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown arg: $1" >&2; usage; exit 2 ;;
  esac
done

if [[ -z "$CLUSTER_NAME" || -z "$LOCATION" ]]; then
  echo "ERROR: --name and --location are required" >&2
  usage
  exit 2
fi

if [[ -z "$SSH_KEY" ]]; then
  for candidate in "$HOME/.ssh/id_ed25519.pub" "$HOME/.ssh/id_rsa.pub"; do
    if [[ -r "$candidate" ]]; then SSH_KEY="$candidate"; break; fi
  done
fi
if [[ -z "$SSH_KEY" || ! -r "$SSH_KEY" ]]; then
  echo "ERROR: no SSH public key found. Pass --ssh-key PATH." >&2
  exit 2
fi

if ! command -v az >/dev/null 2>&1; then
  echo "ERROR: az CLI not found in PATH" >&2
  exit 2
fi

if ! az account show >/dev/null 2>&1; then
  echo "ERROR: not logged in to Azure. Run: az login" >&2
  exit 2
fi

SUBSCRIPTION_ID="$(az account show --query id -o tsv)"
SUBSCRIPTION_NAME="$(az account show --query name -o tsv)"

CIDR_JSON="[]"
if [[ -n "$ALLOWED_SSH_CIDRS" ]]; then
  CIDR_JSON="$(jq -Rcn --arg s "$ALLOWED_SSH_CIDRS" '$s | split(",") | map(select(length>0))')"
fi

SSH_KEY_CONTENT="$(cat "$SSH_KEY")"

if [[ -n "$EXISTING_RG" ]]; then
  echo "==> Ensuring resource group $EXISTING_RG exists in $LOCATION" >&2
  az group create --name "$EXISTING_RG" --location "$LOCATION" --tags azcluster=true "azcluster-name=$CLUSTER_NAME" -o none
fi

cat >&2 <<EOF
==> Deployment plan
    Subscription: $SUBSCRIPTION_NAME ($SUBSCRIPTION_ID)
    Region:       $LOCATION
    Cluster:      $CLUSTER_NAME
    Resource group: ${EXISTING_RG:-rg-azcluster-$CLUSTER_NAME (will be created)}
    SSH key:      $SSH_KEY
    Login public IP: $LOGIN_PUBLIC_IP
    Allowed SSH:  ${ALLOWED_SSH_CIDRS:-(any, since login public IP is off or no narrowing)}
    Ubuntu SKU:   $UBUNTU_SKU
    ANF:          ${ANF_SIZE_TIB} TiB / $ANF_TIER
    Compute:      $COMPUTE_POOL = $COMPUTE_COUNT x $COMPUTE_SKU
    azcluster:    $AZCLUSTER_REPO @ $AZCLUSTER_VERSION
EOF

DEPLOYMENT_NAME="azcluster-${CLUSTER_NAME}-$(date -u +%Y%m%d-%H%M%S)"
BICEP_FILE="$(dirname "$0")/bicep/main.bicep"

ARGS=(
  --name "$DEPLOYMENT_NAME"
  --location "$LOCATION"
  --template-file "$BICEP_FILE"
  --parameters
    clusterName="$CLUSTER_NAME"
    location="$LOCATION"
    sshPublicKey="$SSH_KEY_CONTENT"
    loginPublicIp="$LOGIN_PUBLIC_IP"
    allowedSshCidrs="$CIDR_JSON"
    azclusterVersion="$AZCLUSTER_VERSION"
    azclusterRepo="$AZCLUSTER_REPO"
    ubuntuSku="$UBUNTU_SKU"
    existingResourceGroup="$EXISTING_RG"
    anfSizeTiB="$ANF_SIZE_TIB"
    anfServiceLevel="$ANF_TIER"
    computePoolName="$COMPUTE_POOL"
    computeSku="$COMPUTE_SKU"
    computeCount="$COMPUTE_COUNT"
)

if [[ "$WHAT_IF" == "true" ]]; then
  echo "==> Running az deployment sub what-if" >&2
  az deployment sub what-if "${ARGS[@]}"
  exit 0
fi

echo "==> Running az deployment sub create" >&2
az deployment sub create "${ARGS[@]}"

echo "==> Outputs:" >&2
az deployment sub show --name "$DEPLOYMENT_NAME" --query properties.outputs
