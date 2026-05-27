targetScope = 'resourceGroup'

param clusterName string
param location string
@secure()
param sshPublicKey string
param adminUsername string
param schedulerSku string
param loginSku string
param ubuntuSku string
param loginPublicIp bool
param allowedSshCidrs array
param azclusterVersion string
param azclusterRepo string
param vnetAddressPrefix string
param anfSizeTiB int
param anfServiceLevel string
param amlfsSizeTiB int
param amlfsSkuName string
param amlfsZone string
param pools array
param enableMonitoring bool
param grafanaLocation string
param deployerPrincipalId string
param deployerPrincipalType string = 'User'
param keyVaultName string
param enableStorage bool = true
param storageAccountName string = ''
param storageHns bool = false
param storagePublicAccess bool = false
param storageSku string = 'Standard_LRS'
param storageAccessTier string = 'Hot'
param azcpVersion string = 'v0.4.5'
param sharedStorageMode string = 'anf'
param enableAccounting bool = false
@secure()
param mysqlAdminPassword string = ''
@secure()
param ldapAdminPassword string
param extraPackages string = ''
param enableBastion bool = false
param tags object

module network 'modules/network.bicep' = {
  name: 'network'
  params: {
    clusterName: clusterName
    location: location
    vnetAddressPrefix: vnetAddressPrefix
    allowedSshCidrs: allowedSshCidrs
    enableBastion: enableBastion
    tags: tags
  }
}

module bastion 'modules/bastion.bicep' = if (enableBastion) {
  name: 'bastion'
  params: {
    clusterName: clusterName
    location: location
    subnetId: network.outputs.bastionSubnetId
    tags: tags
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

resource uai 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: 'uai-${clusterName}-scheduler'
  location: location
  tags: tags
}

var contributorRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b24988ac-6180-42a0-ab88-20f7382dd24c')

resource schedulerContributor 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(resourceGroup().id, uai.id, contributorRoleId)
  scope: resourceGroup()
  properties: {
    principalId: uai.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: contributorRoleId
  }
}

module storage 'modules/storage.bicep' = if (enableStorage) {
  name: 'storage'
  params: {
    storageAccountName: storageAccountName
    location: location
    enableHns: storageHns
    sku: storageSku
    accessTier: storageAccessTier
    allowPublicAccess: storagePublicAccess
    uaiPrincipalId: uai.properties.principalId
    peSubnetId: network.outputs.computeSubnetId
    vnetId: network.outputs.vnetId
    tags: tags
  }
}

module anf 'modules/anf.bicep' = if (sharedStorageMode == 'anf') {
  name: 'anf'
  params: {
    clusterName: clusterName
    location: location
    subnetId: network.outputs.anfSubnetId
    sizeTiB: anfSizeTiB
    serviceLevel: anfServiceLevel
    tags: tags
  }
}

var runNfsServerOnScheduler = sharedStorageMode == 'nfs-scheduler'

module amlfs 'modules/amlfs.bicep' = if (amlfsSizeTiB > 0) {
  name: 'amlfs'
  params: {
    clusterName: clusterName
    location: location
    subnetId: network.outputs.amlfsSubnetId
    sizeTiB: amlfsSizeTiB
    skuName: amlfsSkuName
    zone: amlfsZone
    tags: tags
  }
}

var partitionsConf = join(map(pools, p => 'NodeSet=${p.name}set Feature=pool_${p.name}\n      PartitionName=${p.name} Nodes=${p.name}set State=UP MaxTime=INFINITE${p.?default == true ? ' Default=YES' : ''}'), '\n      ')

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

module accounting 'modules/accounting.bicep' = if (enableAccounting) {
  name: 'accounting'
  params: {
    clusterName: clusterName
    location: location
    delegatedSubnetId: network.outputs.databaseSubnetId
    adminPassword: mysqlAdminPassword
    tags: tags
  }
}

module scheduler 'modules/scheduler.bicep' = {
  name: 'scheduler'
  dependsOn: [
    schedulerContributor
  ]
  params: {
    clusterName: clusterName
    location: location
    vmSku: schedulerSku
    ubuntuSku: ubuntuSku
    subnetId: network.outputs.schedulerSubnetId
    sshPublicKey: sshPublicKey
    adminUsername: adminUsername
    azclusterVersion: azclusterVersion
    azclusterRepo: azclusterRepo
    sharedMountIp: runNfsServerOnScheduler ? '' : anf!.outputs.mountIp
    sharedExportPath: runNfsServerOnScheduler ? '' : anf!.outputs.mountPath
    runNfsServer: runNfsServerOnScheduler
    partitionsConf: partitionsConf
    userAssignedIdentityId: uai.id
    userAssignedIdentityClientId: uai.properties.clientId
    monUaiId: enableMonitoring ? monitoring!.outputs.monUaiId : ''
    monUaiClientId: enableMonitoring ? monitoring!.outputs.monUaiClientId : ''
    amwIngestionEndpoint: enableMonitoring ? monitoring!.outputs.ingestionEndpoint : ''
    enableAccounting: enableAccounting
    accountingMysqlFqdn: enableAccounting ? accounting!.outputs.fqdn : ''
    accountingMysqlUser: enableAccounting ? accounting!.outputs.adminLogin : ''
    accountingMysqlDatabase: enableAccounting ? accounting!.outputs.databaseName : ''
    mysqlAdminPassword: mysqlAdminPassword
    ldapAdminPassword: ldapAdminPassword
    extraPackages: extraPackages
    storageAccountName: enableStorage ? storage!.outputs.storageAccountName : ''
    storageBlobEndpoint: enableStorage ? storage!.outputs.blobEndpoint : ''
    storageDataContainerUrl: enableStorage ? storage!.outputs.dataContainerUrl : ''
    azcpVersion: azcpVersion
    tags: tags
  }
}

var sharedMountIpEffective = runNfsServerOnScheduler ? scheduler.outputs.privateIp : anf!.outputs.mountIp
var sharedExportPathEffective = runNfsServerOnScheduler ? 'shared' : anf!.outputs.mountPath

module login 'modules/login.bicep' = {
  name: 'login'
  params: {
    clusterName: clusterName
    location: location
    vmSku: loginSku
    ubuntuSku: ubuntuSku
    subnetId: network.outputs.loginSubnetId
    sshPublicKey: sshPublicKey
    adminUsername: adminUsername
    publicIp: loginPublicIp
    schedulerPrivateIp: scheduler.outputs.privateIp
    sharedMountIp: sharedMountIpEffective
    sharedExportPath: sharedExportPathEffective
    azclusterVersion: azclusterVersion
    azclusterRepo: azclusterRepo
    amlfsMountCommand: amlfsSizeTiB > 0 ? amlfs.outputs.mountCommand : ''
    monUaiId: enableMonitoring ? monitoring!.outputs.monUaiId : ''
    monUaiClientId: enableMonitoring ? monitoring!.outputs.monUaiClientId : ''
    amwIngestionEndpoint: enableMonitoring ? monitoring!.outputs.ingestionEndpoint : ''
    extraPackages: extraPackages
    storageAccountName: enableStorage ? storage!.outputs.storageAccountName : ''
    storageBlobEndpoint: enableStorage ? storage!.outputs.blobEndpoint : ''
    storageDataContainerUrl: enableStorage ? storage!.outputs.dataContainerUrl : ''
    azcpVersion: azcpVersion
    clusterUaiId: uai.id
    clusterUaiClientId: uai.properties.clientId
    tags: tags
  }
}

module compute 'modules/compute.bicep' = [for pool in pools: {
  name: 'compute-${pool.name}'
  params: {
    clusterName: clusterName
    poolName: pool.name
    location: location
    vmSku: pool.sku
    ubuntuSku: ubuntuSku
    subnetId: network.outputs.computeSubnetId
    sshPublicKey: sshPublicKey
    adminUsername: adminUsername
    desiredCount: pool.count
    azclusterVersion: azclusterVersion
    azclusterRepo: azclusterRepo
    schedulerPrivateIp: scheduler.outputs.privateIp
    sharedMountIp: sharedMountIpEffective
    sharedExportPath: sharedExportPathEffective
    amlfsMountCommand: amlfsSizeTiB > 0 ? amlfs.outputs.mountCommand : ''
    monUaiId: enableMonitoring ? monitoring!.outputs.monUaiId : ''
    monUaiClientId: enableMonitoring ? monitoring!.outputs.monUaiClientId : ''
    amwIngestionEndpoint: enableMonitoring ? monitoring!.outputs.ingestionEndpoint : ''
    extraPackages: extraPackages
    storageAccountName: enableStorage ? storage!.outputs.storageAccountName : ''
    storageBlobEndpoint: enableStorage ? storage!.outputs.blobEndpoint : ''
    storageDataContainerUrl: enableStorage ? storage!.outputs.dataContainerUrl : ''
    azcpVersion: azcpVersion
    clusterUaiId: uai.id
    clusterUaiClientId: uai.properties.clientId
    tags: tags
  }
}]

output loginPublicIp string = login.outputs.publicIp
output schedulerPrivateIp string = scheduler.outputs.privateIp
output anfMountIp string = sharedMountIpEffective
output amlfsMgsAddress string = amlfsSizeTiB > 0 ? amlfs.outputs.mgsAddress : ''
output amlfsMountCommand string = amlfsSizeTiB > 0 ? amlfs.outputs.mountCommand : ''
output computeVmssNames array = [for (pool, i) in pools: compute[i].outputs.vmssName]
output grafanaEndpoint string = enableMonitoring ? monitoring!.outputs.grafanaEndpoint : ''
output grafanaName string = enableMonitoring ? monitoring!.outputs.grafanaName : ''
output bastionName string = enableBastion ? bastion!.outputs.bastionName : ''
output bastionDnsName string = enableBastion ? bastion!.outputs.bastionDnsName : ''
output bastionId string = enableBastion ? bastion!.outputs.bastionId : ''
output keyVaultName string = keyvault.outputs.vaultName
output keyVaultUri string = keyvault.outputs.vaultUri
output keyVaultId string = keyvault.outputs.vaultId
output storageAccountName string = enableStorage ? storage!.outputs.storageAccountName : ''
output storageBlobEndpoint string = enableStorage ? storage!.outputs.blobEndpoint : ''
output storageDfsEndpoint string = enableStorage ? storage!.outputs.dfsEndpoint : ''
output storageDataContainerUrl string = enableStorage ? storage!.outputs.dataContainerUrl : ''

