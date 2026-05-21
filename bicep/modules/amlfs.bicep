param clusterName string
param location string
param subnetId string
param sizeTiB int
@allowed([
  'AMLFS-Durable-Premium-40'
  'AMLFS-Durable-Premium-125'
  'AMLFS-Durable-Premium-250'
  'AMLFS-Durable-Premium-500'
])
param skuName string
param zone string
param tags object

resource fs 'Microsoft.StorageCache/amlFilesystems@2024-03-01' = {
  name: 'amlfs-${clusterName}'
  location: location
  tags: tags
  sku: {
    name: skuName
  }
  zones: [
    zone
  ]
  properties: {
    filesystemSubnet: subnetId
    storageCapacityTiB: sizeTiB
    maintenanceWindow: {
      dayOfWeek: 'Sunday'
      timeOfDayUTC: '03:00'
    }
  }
}

output mgsAddress string = fs.properties.clientInfo.mgsAddress
output mountCommand string = fs.properties.clientInfo.mountCommand
