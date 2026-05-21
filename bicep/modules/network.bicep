param clusterName string
param location string
param vnetAddressPrefix string
param allowedSshCidrs array
param tags object

var effectiveSshCidrs = empty(allowedSshCidrs) ? [ 'Internet' ] : allowedSshCidrs

resource loginNsg 'Microsoft.Network/networkSecurityGroups@2024-01-01' = {
  name: 'nsg-${clusterName}-login'
  location: location
  tags: tags
  properties: {
    securityRules: [for (cidr, i) in effectiveSshCidrs: {
      name: 'allow-ssh-${i}'
      properties: {
        priority: 1000 + i
        access: 'Allow'
        direction: 'Inbound'
        protocol: 'Tcp'
        sourceAddressPrefix: cidr
        sourcePortRange: '*'
        destinationAddressPrefix: '*'
        destinationPortRange: '22'
      }
    }]
  }
}

resource internalNsg 'Microsoft.Network/networkSecurityGroups@2024-01-01' = {
  name: 'nsg-${clusterName}-internal'
  location: location
  tags: tags
  properties: {
    securityRules: [
      {
        name: 'allow-vnet-inbound'
        properties: {
          priority: 1000
          access: 'Allow'
          direction: 'Inbound'
          protocol: '*'
          sourceAddressPrefix: 'VirtualNetwork'
          sourcePortRange: '*'
          destinationAddressPrefix: 'VirtualNetwork'
          destinationPortRange: '*'
        }
      }
    ]
  }
}

resource natPip 'Microsoft.Network/publicIPAddresses@2024-01-01' = {
  name: 'pip-${clusterName}-nat'
  location: location
  tags: tags
  sku: {
    name: 'Standard'
  }
  properties: {
    publicIPAllocationMethod: 'Static'
  }
}

resource natGw 'Microsoft.Network/natGateways@2024-01-01' = {
  name: 'natgw-${clusterName}'
  location: location
  tags: tags
  sku: {
    name: 'Standard'
  }
  properties: {
    idleTimeoutInMinutes: 10
    publicIpAddresses: [
      {
        id: natPip.id
      }
    ]
  }
}

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
        name: 'scheduler'
        properties: {
          addressPrefix: cidrSubnet(vnetAddressPrefix, 24, 1)
          networkSecurityGroup: {
            id: internalNsg.id
          }
          natGateway: {
            id: natGw.id
          }
        }
      }
      {
        name: 'login'
        properties: {
          addressPrefix: cidrSubnet(vnetAddressPrefix, 24, 2)
          networkSecurityGroup: {
            id: loginNsg.id
          }
          natGateway: {
            id: natGw.id
          }
        }
      }
      {
        name: 'compute'
        properties: {
          addressPrefix: cidrSubnet(vnetAddressPrefix, 22, 1)
          networkSecurityGroup: {
            id: internalNsg.id
          }
          natGateway: {
            id: natGw.id
          }
        }
      }
      {
        name: 'anf'
        properties: {
          addressPrefix: cidrSubnet(vnetAddressPrefix, 26, 0)
          delegations: [
            {
              name: 'netapp'
              properties: {
                serviceName: 'Microsoft.Netapp/volumes'
              }
            }
          ]
        }
      }
      {
        name: 'amlfs'
        properties: {
          addressPrefix: cidrSubnet(vnetAddressPrefix, 24, 3)
        }
      }
    ]
  }
}

output schedulerSubnetId string = '${vnet.id}/subnets/scheduler'
output loginSubnetId string = '${vnet.id}/subnets/login'
output computeSubnetId string = '${vnet.id}/subnets/compute'
output anfSubnetId string = '${vnet.id}/subnets/anf'
output amlfsSubnetId string = '${vnet.id}/subnets/amlfs'
