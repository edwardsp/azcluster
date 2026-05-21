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
param computePoolName string
param computeSku string
param computeCount int
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

module scheduler 'modules/scheduler.bicep' = {
  name: 'scheduler'
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
    computePoolName: computePoolName
    computeSku: computeSku
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
    tags: tags
  }
}

module compute 'modules/compute.bicep' = {
  name: 'compute-${computePoolName}'
  params: {
    clusterName: clusterName
    poolName: computePoolName
    location: location
    vmSku: computeSku
    ubuntuSku: ubuntuSku
    subnetId: network.outputs.computeSubnetId
    sshPublicKey: sshPublicKey
    adminUsername: adminUsername
    desiredCount: computeCount
    azclusterVersion: azclusterVersion
    azclusterRepo: azclusterRepo
    schedulerPrivateIp: scheduler.outputs.privateIp
    anfMountIp: anf.outputs.mountIp
    anfExportPath: anf.outputs.mountPath
    scalePrincipalId: scheduler.outputs.principalId
    tags: tags
  }
}

output loginPublicIp string = login.outputs.publicIp
output schedulerPrivateIp string = scheduler.outputs.privateIp
output anfMountIp string = anf.outputs.mountIp
output computeVmssName string = compute.outputs.vmssName
