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
param sharedMountIp string
param sharedExportPath string
param runNfsServer bool = false
param partitionsConf string
param userAssignedIdentityId string
param userAssignedIdentityClientId string
param monUaiId string = ''
param monUaiClientId string = ''
param amwIngestionEndpoint string = ''
param enableAccounting bool = false
param accountingMysqlFqdn string = ''
param accountingMysqlUser string = ''
param accountingMysqlDatabase string = ''
@secure()
param mysqlAdminPassword string = ''
@secure()
param ldapAdminPassword string
param extraPackages string = ''
param storageAccountName string = ''
param storageBlobEndpoint string = ''
param storageDataContainerUrl string = ''
param azcpVersion string = 'v0.4.5'
param tags object

var cloudInitTemplate = loadTextContent('../../cloud-init/scheduler.yaml.tmpl')
var cloudInit = replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(cloudInitTemplate,
    '{{AZCLUSTER_VERSION}}', azclusterVersion),
    '{{AZCLUSTER_REPO}}', azclusterRepo),
    '{{ADMIN_USER}}', adminUsername),
    '{{CLUSTER_NAME}}', clusterName),
    '{{SHARED_MOUNT_IP}}', sharedMountIp),
    '{{SHARED_EXPORT_PATH}}', sharedExportPath),
    '{{RUN_NFS_SERVER}}', runNfsServer ? 'true' : 'false'),
    '{{PARTITIONS}}', partitionsConf),
    '{{UAI_CLIENT_ID}}', userAssignedIdentityClientId),
    '{{MON_UAI_CLIENT_ID}}', monUaiClientId),
    '{{AMW_INGEST_URL}}', amwIngestionEndpoint),
    '{{SUBSCRIPTION_ID}}', subscription().subscriptionId),
    '{{ENABLE_ACCOUNTING}}', enableAccounting ? 'true' : 'false'),
    '{{ACCT_MYSQL_FQDN}}', accountingMysqlFqdn),
    '{{ACCT_MYSQL_USER}}', accountingMysqlUser),
    '{{ACCT_MYSQL_DB}}', accountingMysqlDatabase),
    '{{ACCT_MYSQL_PASSWORD}}', mysqlAdminPassword),
    '{{LDAP_ADMIN_PASSWORD}}', ldapAdminPassword),
    '{{EXTRA_PACKAGES}}', extraPackages),
    '{{STORAGE_ACCOUNT_NAME}}', storageAccountName),
    '{{STORAGE_BLOB_ENDPOINT}}', storageBlobEndpoint),
    '{{STORAGE_DATA_CONTAINER_URL}}', storageDataContainerUrl),
    '{{AZCP_VERSION}}', azcpVersion)

var userIdentities = empty(monUaiId) ? {
  '${userAssignedIdentityId}': {}
} : {
  '${userAssignedIdentityId}': {}
  '${monUaiId}': {}
}

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
  identity: {
    type: 'SystemAssigned, UserAssigned'
    userAssignedIdentities: userIdentities
  }
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
output vmName string = vm.name
