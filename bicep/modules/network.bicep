param clusterName string
param location string
param vnetAddressPrefix string
param allowedSshCidrs array
param enableBastion bool = false
param tags object

var baseSubnets = [
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
  {
    name: 'database'
    properties: {
      addressPrefix: cidrSubnet(vnetAddressPrefix, 29, 256)
      delegations: [
        {
          name: 'mysql-flex'
          properties: {
            serviceName: 'Microsoft.DBforMySQL/flexibleServers'
          }
        }
      ]
      serviceEndpoints: [
        {
          service: 'Microsoft.Storage'
        }
      ]
    }
  }
]

// AzureBastionSubnet name is MANDATORY (Azure rejects any other name for Bastion).
// /26 minimum required by Azure Bastion. cidrSubnet(prefix, 26, 1) lands at 10.42.0.64/26
// for the default 10.42.0.0/16 VNet (just above the /26 anf subnet at 10.42.0.0/26).
var bastionSubnets = enableBastion ? [
  {
    name: 'AzureBastionSubnet'
    properties: {
      addressPrefix: cidrSubnet(vnetAddressPrefix, 26, 1)
    }
  }
] : []

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
    subnets: concat(baseSubnets, bastionSubnets)
  }
}

output schedulerSubnetId string = '${vnet.id}/subnets/scheduler'
output loginSubnetId string = '${vnet.id}/subnets/login'
output computeSubnetId string = '${vnet.id}/subnets/compute'
output anfSubnetId string = '${vnet.id}/subnets/anf'
output amlfsSubnetId string = '${vnet.id}/subnets/amlfs'
output databaseSubnetId string = '${vnet.id}/subnets/database'
output bastionSubnetId string = enableBastion ? '${vnet.id}/subnets/AzureBastionSubnet' : ''
output vnetId string = vnet.id
