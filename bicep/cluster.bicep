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
param tags object

module network 'modules/network.bicep' = {
  name: 'network'
  params: {
    clusterName: clusterName
    location: location
    vnetAddressPrefix: vnetAddressPrefix
    allowedSshCidrs: allowedSshCidrs
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

module anf 'modules/anf.bicep' = {
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
    anfMountIp: anf.outputs.mountIp
    anfExportPath: anf.outputs.mountPath
    partitionsConf: partitionsConf
    userAssignedIdentityId: uai.id
    userAssignedIdentityClientId: uai.properties.clientId
    tags: tags
  }
}

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
    anfMountIp: anf.outputs.mountIp
    anfExportPath: anf.outputs.mountPath
    azclusterVersion: azclusterVersion
    azclusterRepo: azclusterRepo
    amlfsMountCommand: amlfsSizeTiB > 0 ? amlfs.outputs.mountCommand : ''
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
    anfMountIp: anf.outputs.mountIp
    anfExportPath: anf.outputs.mountPath
    amlfsMountCommand: amlfsSizeTiB > 0 ? amlfs.outputs.mountCommand : ''
    spot: bool(pool.?spot ?? false)
    spotMaxPrice: pool.?maxPrice ?? '-1'
    tags: tags
  }
}]

output loginPublicIp string = login.outputs.publicIp
output schedulerPrivateIp string = scheduler.outputs.privateIp
output anfMountIp string = anf.outputs.mountIp
output amlfsMgsAddress string = amlfsSizeTiB > 0 ? amlfs.outputs.mgsAddress : ''
output amlfsMountCommand string = amlfsSizeTiB > 0 ? amlfs.outputs.mountCommand : ''
output computeVmssNames array = [for (pool, i) in pools: compute[i].outputs.vmssName]
