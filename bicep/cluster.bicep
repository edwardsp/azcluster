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
    tags: tags
  }
}

output loginPublicIp string = login.outputs.publicIp
output schedulerPrivateIp string = scheduler.outputs.privateIp
