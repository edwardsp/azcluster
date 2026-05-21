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
    tags: commonTags
  }
}

output resourceGroupName string = rgName
output loginPublicIp string = cluster.outputs.loginPublicIp
output schedulerPrivateIp string = cluster.outputs.schedulerPrivateIp
