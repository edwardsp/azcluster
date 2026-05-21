param clusterName string
param poolName string
param location string
param vmSku string
param ubuntuSku string
param subnetId string
@secure()
param sshPublicKey string
param adminUsername string
param desiredCount int
param azclusterVersion string
param azclusterRepo string
param schedulerPrivateIp string
param anfMountIp string
param anfExportPath string
param scalePrincipalId string = ''
param tags object

var cloudInitTemplate = loadTextContent('../../cloud-init/compute.yaml.tmpl')
var cloudInit = replace(replace(replace(replace(replace(replace(replace(cloudInitTemplate,
    '{{AZCLUSTER_VERSION}}', azclusterVersion),
    '{{AZCLUSTER_REPO}}', azclusterRepo),
    '{{ADMIN_USER}}', adminUsername),
    '{{CLUSTER_NAME}}', clusterName),
    '{{SCHEDULER_IP}}', schedulerPrivateIp),
    '{{ANF_MOUNT_IP}}', anfMountIp),
    '{{ANF_EXPORT_PATH}}', anfExportPath)

resource vmss 'Microsoft.Compute/virtualMachineScaleSets@2024-07-01' = {
  name: 'vmss-${clusterName}-${poolName}'
  location: location
  tags: tags
  sku: {
    name: vmSku
    capacity: desiredCount
  }
  properties: {
    orchestrationMode: 'Flexible'
    platformFaultDomainCount: 1
    singlePlacementGroup: false
    virtualMachineProfile: {
      osProfile: {
        computerNamePrefix: '${clusterName}-${poolName}-'
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
        networkApiVersion: '2022-11-01'
        networkInterfaceConfigurations: [
          {
            name: 'nic'
            properties: {
              primary: true
              ipConfigurations: [
                {
                  name: 'ipcfg'
                  properties: {
                    subnet: {
                      id: subnetId
                    }
                  }
                }
              ]
            }
          }
        ]
      }
    }
  }
}

output vmssName string = vmss.name
output vmssId string = vmss.id

var contributorRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b24988ac-6180-42a0-ab88-20f7382dd24c')

resource scaleRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(scalePrincipalId)) {
  name: guid(vmss.id, scalePrincipalId, contributorRoleId)
  scope: vmss
  properties: {
    principalId: scalePrincipalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: contributorRoleId
  }
}
