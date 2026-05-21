targetScope = 'subscription'

@description('Cluster name. Used as resource-group suffix and tag.')
@minLength(2)
@maxLength(20)
param clusterName string

@description('Azure region.')
param location string

@description('Existing resource group name to use. If empty, a new RG named rg-azcluster-<clusterName> is created.')
param existingResourceGroup string = ''

@description('SSH public key for admin user on all VMs.')
@secure()
param sshPublicKey string

@description('Admin username on VMs.')
param adminUsername string = 'azureuser'

@description('Scheduler VM SKU.')
param schedulerSku string = 'Standard_D8as_v5'

@description('Login VM SKU.')
param loginSku string = 'Standard_D4as_v5'

@description('Ubuntu HPC marketplace SKU.')
@allowed([
  '2204'
  '2404'
])
param ubuntuSku string = '2404'

@description('Give login VM a public IP. Default false. Opt-in for testing.')
param loginPublicIp bool = false

@description('Optional NSG narrowing for SSH when login VM has a public IP. Empty means SSH allowed from Internet.')
param allowedSshCidrs array = []

@description('azcluster release tag to fetch binaries/assets from GitHub Releases.')
param azclusterVersion string = 'v0.0.1'

@description('GitHub org/repo to fetch releases from. Phase 0 default is the upstream repo.')
param azclusterRepo string = 'edwardsp/azcluster'

@description('VNet address space.')
param vnetAddressPrefix string = '10.42.0.0/16'

@description('ANF capacity pool size in TiB. Minimum 1 (Premium/Ultra) or 2 (Standard).')
@minValue(1)
param anfSizeTiB int = 2

@description('ANF service level.')
@allowed([
  'Standard'
  'Premium'
  'Ultra'
])
param anfServiceLevel string = 'Standard'

@description('Compute pool name (becomes Slurm partition name).')
param computePoolName string = 'gpu'

@description('Compute VM SKU for the default pool.')
param computeSku string = 'Standard_ND96isr_H200_v5'

@description('Initial VMSS Flex capacity for the compute pool. 0 means pool is provisioned empty.')
@minValue(0)
param computeCount int = 0

var rgName = empty(existingResourceGroup) ? 'rg-azcluster-${clusterName}' : existingResourceGroup
var commonTags = {
  azcluster: 'true'
  'azcluster-name': clusterName
  'azcluster-version': azclusterVersion
}

resource rg 'Microsoft.Resources/resourceGroups@2024-03-01' = if (empty(existingResourceGroup)) {
  name: rgName
  location: location
  tags: commonTags
}

module cluster 'cluster.bicep' = {
  name: 'cluster-${clusterName}'
  scope: resourceGroup(rgName)
  dependsOn: [
    rg
  ]
  params: {
    clusterName: clusterName
    location: location
    sshPublicKey: sshPublicKey
    adminUsername: adminUsername
    schedulerSku: schedulerSku
    loginSku: loginSku
    ubuntuSku: ubuntuSku
    loginPublicIp: loginPublicIp
    allowedSshCidrs: allowedSshCidrs
    azclusterVersion: azclusterVersion
    azclusterRepo: azclusterRepo
    vnetAddressPrefix: vnetAddressPrefix
    anfSizeTiB: anfSizeTiB
    anfServiceLevel: anfServiceLevel
    computePoolName: computePoolName
    computeSku: computeSku
    computeCount: computeCount
    tags: commonTags
  }
}

output resourceGroupName string = rgName
output loginPublicIp string = cluster.outputs.loginPublicIp
output schedulerPrivateIp string = cluster.outputs.schedulerPrivateIp
output anfMountIp string = cluster.outputs.anfMountIp
output computeVmssName string = cluster.outputs.computeVmssName
