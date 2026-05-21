param clusterName string
param location string
param vmSku string
param ubuntuSku string
param subnetId string
@secure()
param sshPublicKey string
param adminUsername string
param azclusterVersion string
param azclusterRepo string
param tags object

var cloudInitTemplate = loadTextContent('../../cloud-init/scheduler.yaml.tmpl')
var cloudInit = replace(replace(replace(replace(cloudInitTemplate,
    '{{AZCLUSTER_VERSION}}', azclusterVersion),
    '{{AZCLUSTER_REPO}}', azclusterRepo),
    '{{ADMIN_USER}}', adminUsername),
    '{{CLUSTER_NAME}}', clusterName)

resource nic 'Microsoft.Network/networkInterfaces@2024-01-01' = {
  name: 'nic-${clusterName}-scheduler'
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
        }
      }
    ]
  }
}

resource vm 'Microsoft.Compute/virtualMachines@2024-07-01' = {
  name: 'vm-${clusterName}-scheduler'
  location: location
  tags: tags
  properties: {
    hardwareProfile: {
      vmSize: vmSku
    }
    osProfile: {
      computerName: '${clusterName}-scheduler'
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

output privateIp string = nic.properties.ipConfigurations[0].properties.privateIPAddress
output vmId string = vm.id
