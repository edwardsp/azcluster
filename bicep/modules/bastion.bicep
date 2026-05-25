param clusterName string
param location string
param subnetId string
param tags object

// Standard SKU + enableTunneling = vgamayunov no-plugin model.
// `wss://{dnsName}/webtunnelv2/{token}` is the per-connection bridge endpoint.
// ~ $140/month opt-in; only deployed when --bastion is passed.

resource bastionPip 'Microsoft.Network/publicIPAddresses@2024-01-01' = {
  name: 'pip-${clusterName}-bastion'
  location: location
  tags: tags
  sku: {
    name: 'Standard'
  }
  properties: {
    publicIPAllocationMethod: 'Static'
  }
}

resource bastion 'Microsoft.Network/bastionHosts@2024-01-01' = {
  name: 'bastion-${clusterName}'
  location: location
  tags: tags
  sku: {
    name: 'Standard'
  }
  properties: {
    enableTunneling: true
    ipConfigurations: [
      {
        name: 'bastion-ipconfig'
        properties: {
          subnet: {
            id: subnetId
          }
          publicIPAddress: {
            id: bastionPip.id
          }
        }
      }
    ]
  }
}

output bastionId string = bastion.id
output bastionDnsName string = bastion.properties.dnsName
output bastionName string = bastion.name
