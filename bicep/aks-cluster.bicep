targetScope = 'resourceGroup'

param clusterName string
param location string
param kubernetesVersion string
param systemNodeSku string
param systemNodeCount int
param gpuPoolName string
param gpuSku string
param gpuNodeCount int
param vnetAddressPrefix string
@secure()
param sshPublicKey string
param adminUsername string
param deployerPrincipalId string
param deployerPrincipalType string = 'User'
param keyVaultName string
param enableMonitoring bool = false
param grafanaLocation string = location
param enableStorage bool = false
param storageAccountName string = ''
param storageSku string = 'Standard_LRS'
param storageAccessTier string = 'Hot'
param tags object

resource vnet 'Microsoft.Network/virtualNetworks@2024-01-01' = {
  name: 'vnet-${clusterName}'
  location: location
  tags: tags
  properties: {
    addressSpace: {
      addressPrefixes: [
        vnetAddressPrefix
      ]
    }
    subnets: [
      {
        name: 'aks-nodes'
        properties: {
          addressPrefix: cidrSubnet(vnetAddressPrefix, 20, 1)
        }
      }
    ]
  }
}

module keyvault 'modules/keyvault.bicep' = {
  name: 'keyvault'
  params: {
    keyVaultName: keyVaultName
    location: location
    deployerPrincipalId: deployerPrincipalId
    deployerPrincipalType: deployerPrincipalType
    tags: tags
  }
}

module monitoring 'modules/monitoring.bicep' = if (enableMonitoring) {
  name: 'monitoring'
  params: {
    clusterName: clusterName
    location: location
    grafanaLocation: grafanaLocation
    deployerPrincipalId: deployerPrincipalId
    deployerPrincipalType: deployerPrincipalType
    tags: tags
  }
}

module aks 'modules/aks.bicep' = {
  name: 'aks'
  params: {
    clusterName: clusterName
    location: location
    kubernetesVersion: kubernetesVersion
    systemNodeSku: systemNodeSku
    systemNodeCount: systemNodeCount
    gpuPoolName: gpuPoolName
    gpuSku: gpuSku
    gpuNodeCount: gpuNodeCount
    subnetId: '${vnet.id}/subnets/aks-nodes'
    sshPublicKey: sshPublicKey
    adminUsername: adminUsername
    enableMonitoring: enableMonitoring
    enableStorage: enableStorage
    tags: tags
  }
}

// TODO(M5): wire azureMonitorProfile DCR association into the reused AMW.
var grafanaEndpoint = enableMonitoring ? monitoring!.outputs.grafanaEndpoint : ''

// Per-cluster Blob account for the blob-first data flow (training data + checkpoints
// staged to Blob, consumed via azcp/blobfuse). The AKS kubelet (node) identity is
// granted Storage Blob Data Contributor so in-pod azcp authenticates through IMDS.
module storage 'modules/storage.bicep' = if (enableStorage) {
  name: 'storage'
  params: {
    storageAccountName: storageAccountName
    location: location
    enableHns: false
    sku: storageSku
    accessTier: storageAccessTier
    allowPublicAccess: false
    uaiPrincipalId: aks.outputs.kubeletIdentityObjectId
    peSubnetId: '${vnet.id}/subnets/aks-nodes'
    vnetId: vnet.id
    tags: tags
  }
}

output aksClusterName string = aks.outputs.aksClusterName
output nodeResourceGroup string = aks.outputs.nodeResourceGroup
output fqdn string = aks.outputs.fqdn
output kubeletIdentityObjectId string = aks.outputs.kubeletIdentityObjectId
output oidcIssuerUrl string = aks.outputs.oidcIssuerUrl
output gpuPoolName string = aks.outputs.gpuPoolName
output gpuSku string = aks.outputs.gpuSku
output gpuNodeCount int = aks.outputs.gpuNodeCount
output grafanaEndpoint string = grafanaEndpoint
output keyVaultName string = keyvault.outputs.vaultName
output keyVaultUri string = keyvault.outputs.vaultUri
output keyVaultId string = keyvault.outputs.vaultId
output storageAccountName string = enableStorage ? storage!.outputs.storageAccountName : ''
output storageBlobEndpoint string = enableStorage ? storage!.outputs.blobEndpoint : ''
output storageDataContainerUrl string = enableStorage ? storage!.outputs.dataContainerUrl : ''
