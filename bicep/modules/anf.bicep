param clusterName string
param location string
param subnetId string
param sizeTiB int
@allowed([
  'Standard'
  'Premium'
  'Ultra'
])
param serviceLevel string
param tags object

var sizeBytes = sizeTiB * 1024 * 1024 * 1024 * 1024
var accountName = 'anf${uniqueString(resourceGroup().id, clusterName)}'
var poolName = 'pool1'
var volumeName = 'shared'

resource account 'Microsoft.NetApp/netAppAccounts@2024-03-01' = {
  name: accountName
  location: location
  tags: tags
  properties: {}
}

resource pool 'Microsoft.NetApp/netAppAccounts/capacityPools@2024-03-01' = {
  parent: account
  name: poolName
  location: location
  tags: tags
  properties: {
    serviceLevel: serviceLevel
    size: sizeBytes
    qosType: 'Auto'
  }
}

resource volume 'Microsoft.NetApp/netAppAccounts/capacityPools/volumes@2024-03-01' = {
  parent: pool
  name: volumeName
  location: location
  tags: tags
  properties: {
    serviceLevel: serviceLevel
    creationToken: '${clusterName}-shared'
    usageThreshold: sizeBytes
    subnetId: subnetId
    protocolTypes: [
      'NFSv4.1'
    ]
    exportPolicy: {
      rules: [
        {
          ruleIndex: 1
          unixReadOnly: false
          unixReadWrite: true
          cifs: false
          nfsv3: false
          nfsv41: true
          allowedClients: '0.0.0.0/0'
          hasRootAccess: true
          kerberos5ReadOnly: false
          kerberos5ReadWrite: false
          kerberos5iReadOnly: false
          kerberos5iReadWrite: false
          kerberos5pReadOnly: false
          kerberos5pReadWrite: false
        }
      ]
    }
    securityStyle: 'unix'
    snapshotDirectoryVisible: true
  }
}

output mountIp string = volume.properties.mountTargets[0].ipAddress
output mountPath string = volume.properties.creationToken
