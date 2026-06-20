param clusterName string
param location string
param kubernetesVersion string
param systemNodeSku string
param systemNodeCount int
param gpuPoolName string
param gpuSku string
param gpuNodeCount int
param subnetId string
@secure()
param sshPublicKey string
param adminUsername string
param enableMonitoring bool = false
param enableStorage bool = false
param tags object

var aksName = 'aks-${clusterName}'
var dnsPrefix = take(replace(replace(toLower('${clusterName}-aks'), '_', '-'), '.', '-'), 54)

resource aks 'Microsoft.ContainerService/managedClusters@2025-10-01' = {
  name: aksName
  location: location
  tags: tags
  identity: {
    type: 'SystemAssigned'
  }
  properties: union(empty(kubernetesVersion) ? {} : { kubernetesVersion: kubernetesVersion }, {
    dnsPrefix: dnsPrefix
    enableRBAC: true
    disableLocalAccounts: false
    networkProfile: {
      networkPlugin: 'azure'
      loadBalancerSku: 'standard'
    }
    oidcIssuerProfile: {
      enabled: true
    }
    azureMonitorProfile: {
      metrics: {
        enabled: enableMonitoring
      }
    }
    storageProfile: {
      blobCSIDriver: {
        enabled: enableStorage
      }
    }
    agentPoolProfiles: [
      {
        name: 'system'
        mode: 'System'
        vmSize: systemNodeSku
        count: systemNodeCount
        vnetSubnetID: subnetId
        osType: 'Linux'
        osSKU: 'Ubuntu'
        type: 'VirtualMachineScaleSets'
      }
    ]
    linuxProfile: {
      adminUsername: adminUsername
      ssh: {
        publicKeys: [
          {
            keyData: sshPublicKey
          }
        ]
      }
    }
  })
}

resource gpuPool 'Microsoft.ContainerService/managedClusters/agentPools@2025-10-01' = {
  parent: aks
  name: gpuPoolName
  properties: {
    mode: 'User'
    vmSize: gpuSku
    count: gpuNodeCount
    vnetSubnetID: subnetId
    osType: 'Linux'
    osSKU: 'Ubuntu'
    type: 'VirtualMachineScaleSets'
    enableNodePublicIP: false
    // Azure CNI defaults to maxPods=30; the GPU/network/storage operator
    // daemonsets + ama-metrics + kube-system already consume ~28-30 slots,
    // leaving no room for an 8-GPU workload pod. 110 gives ample headroom.
    maxPods: 110
    nodeLabels: {
      agentpool: gpuPoolName
    }
    // GPU Operator owns driver lifecycle; gpuProfile.driver=None skips the AKS-managed driver.
    gpuProfile: {
      driver: 'None'
    }
    // ND-series InfiniBand is exposed automatically after subscription feature registration.
  }
}

output aksClusterName string = aks.name
output nodeResourceGroup string = aks.properties.nodeResourceGroup
output fqdn string = aks.properties.fqdn
output kubeletIdentityObjectId string = aks.properties.?identityProfile.?kubeletidentity.?objectId ?? ''
output kubeletIdentityClientId string = aks.properties.?identityProfile.?kubeletidentity.?clientId ?? ''
output oidcIssuerUrl string = aks.properties.oidcIssuerProfile.issuerURL
output gpuPoolName string = gpuPoolName
output gpuSku string = gpuSku
output gpuNodeCount int = gpuNodeCount
