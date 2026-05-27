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
param sharedMountIp string
param sharedExportPath string
param amlfsMountCommand string
param monUaiId string = ''
param monUaiClientId string = ''
param amwIngestionEndpoint string = ''
param extraPackages string = ''
param storageAccountName string = ''
param storageBlobEndpoint string = ''
param storageDataContainerUrl string = ''
param azcpVersion string = 'v0.4.5'
param clusterUaiId string = ''
param clusterUaiClientId string = ''
param tags object

var cloudInitTemplate = loadTextContent('../../cloud-init/compute.yaml.tmpl')
var cloudInit = replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(cloudInitTemplate,
    '{{AZCLUSTER_VERSION}}', azclusterVersion),
    '{{AZCLUSTER_REPO}}', azclusterRepo),
    '{{ADMIN_USER}}', adminUsername),
    '{{CLUSTER_NAME}}', clusterName),
    '{{POOL_NAME}}', poolName),
    '{{SCHEDULER_IP}}', schedulerPrivateIp),
    '{{SHARED_MOUNT_IP}}', sharedMountIp),
    '{{SHARED_EXPORT_PATH}}', sharedExportPath),
    '{{AMLFS_MOUNT_CMD}}', amlfsMountCommand),
    '{{MON_UAI_CLIENT_ID}}', monUaiClientId),
    '{{AMW_INGEST_URL}}', amwIngestionEndpoint),
    '{{SUBSCRIPTION_ID}}', subscription().subscriptionId),
    '{{EXTRA_PACKAGES}}', extraPackages),
    '{{STORAGE_ACCOUNT_NAME}}', storageAccountName),
    '{{STORAGE_BLOB_ENDPOINT}}', storageBlobEndpoint),
    '{{STORAGE_DATA_CONTAINER_URL}}', storageDataContainerUrl),
    '{{AZCP_VERSION}}', azcpVersion),
    '{{UAI_CLIENT_ID}}', clusterUaiClientId)

var hasMonUai = !empty(monUaiId)
var hasClusterUai = !empty(clusterUaiId)
var combinedUaiIds = union(
  hasMonUai ? { '${monUaiId}': {} } : {},
  hasClusterUai ? { '${clusterUaiId}': {} } : {}
)
var anyUai = hasMonUai || hasClusterUai

resource vmss 'Microsoft.Compute/virtualMachineScaleSets@2024-07-01' = {
  name: 'vmss-${clusterName}-${poolName}'
  location: location
  tags: tags
  identity: anyUai ? {
    type: 'UserAssigned'
    userAssignedIdentities: combinedUaiIds
  } : null
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
