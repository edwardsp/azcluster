param clusterName string
param location string
param vmSku string
param ubuntuSku string
param subnetId string
@secure()
param sshPublicKey string
param adminUsername string
param publicIp bool
param schedulerPrivateIp string
param tags object

var cloudInitTemplate = loadTextContent('../../cloud-init/login.yaml.tmpl')
var cloudInit = replace(replace(replace(cloudInitTemplate,
    '{{ADMIN_USER}}', adminUsername),
    '{{CLUSTER_NAME}}', clusterName),
    '{{SCHEDULER_IP}}', schedulerPrivateIp)

resource pip 'Microsoft.Network/publicIPAddresses@2024-01-01' = if (publicIp) {
  name: 'pip-${clusterName}-login'
  location: location
  tags: tags
  sku: {
    name: 'Standard'
  }
  properties: {
    publicIPAllocationMethod: 'Static'
    publicIPAddressVersion: 'IPv4'
  }
}

resource nic 'Microsoft.Network/networkInterfaces@2024-01-01' = {
  name: 'nic-${clusterName}-login'
  location: location
  tags: tags
  properties: {
    ipConfigurations: [
      {
        name: 'ipcfg'
        properties: {
          privateIPAllocationMethod: 'Dynamic'
          subnet: {
            id: subnetId
          }
          publicIPAddress: publicIp ? {
            id: pip.id
          } : null
        }
      }
    ]
  }
}

resource vm 'Microsoft.Compute/virtualMachines@2024-07-01' = {
  name: 'vm-${clusterName}-login'
  location: location
  tags: tags
  properties: {
    hardwareProfile: {
      vmSize: vmSku
    }
    osProfile: {
      computerName: '${clusterName}-login'
      adminUsername: adminUsername
      linuxConfiguration: {
        disablePasswordAuthentication: true
        ssh: {
          publicKeys: [
            {
              path: '/home/${adminUsername}/.ssh/authorized_keys'
              keyData: sshPublicKey
            }
          ]
        }
      }
      customData: base64(cloudInit)
    }
    storageProfile: {
      imageReference: {
        publisher: 'microsoft-dsvm'
        offer: 'ubuntu-hpc'
        sku: ubuntuSku
        version: 'latest'
      }
      osDisk: {
        createOption: 'FromImage'
        managedDisk: {
          storageAccountType: 'Premium_LRS'
        }
      }
    }
    networkProfile: {
      networkInterfaces: [
        {
          id: nic.id
        }
      ]
    }
  }
}

output publicIp string = publicIp ? pip!.properties.ipAddress : ''
output privateIp string = nic.properties.ipConfigurations[0].properties.privateIPAddress
output vmId string = vm.id
