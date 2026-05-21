param clusterName string
param location string
param schedulerVmName string
param loginVmName string
param computeVmssNames array
param tags object

var amwName = 'amw-${clusterName}'
var grafanaName = 'amg-${clusterName}'
var monitoringDataReaderRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b0d8363b-78d5-41c0-9c38-6abe57b51537')
var metricsPublisherRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '3913510d-42f4-4e42-8a64-420c390055eb')

resource amw 'Microsoft.Monitor/accounts@2023-04-03' = {
  name: amwName
  location: location
  tags: tags
}

resource schedulerVm 'Microsoft.Compute/virtualMachines@2024-07-01' existing = {
  name: schedulerVmName
}

resource loginVm 'Microsoft.Compute/virtualMachines@2024-07-01' existing = {
  name: loginVmName
}

resource computeVmss 'Microsoft.Compute/virtualMachineScaleSets@2024-07-01' existing = [for name in computeVmssNames: {
  name: name
}]

resource raScheduler 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: amw
  name: guid(amw.id, schedulerVm.id, metricsPublisherRoleId)
  properties: {
    roleDefinitionId: metricsPublisherRoleId
    principalId: schedulerVm.identity.principalId
    principalType: 'ServicePrincipal'
  }
}

resource raLogin 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: amw
  name: guid(amw.id, loginVm.id, metricsPublisherRoleId)
  properties: {
    roleDefinitionId: metricsPublisherRoleId
    principalId: loginVm.identity.principalId
    principalType: 'ServicePrincipal'
  }
}

resource raCompute 'Microsoft.Authorization/roleAssignments@2022-04-01' = [for (name, i) in computeVmssNames: {
  scope: amw
  name: guid(amw.id, computeVmss[i].id, metricsPublisherRoleId)
  properties: {
    roleDefinitionId: metricsPublisherRoleId
    principalId: computeVmss[i].identity.principalId
    principalType: 'ServicePrincipal'
  }
}]

resource grafana 'Microsoft.Dashboard/grafana@2024-10-01' = {
  name: grafanaName
  location: location
  tags: tags
  sku: {
    name: 'Standard'
  }
  identity: {
    type: 'SystemAssigned'
  }
  properties: {
    apiKey: 'Enabled'
    publicNetworkAccess: 'Enabled'
    grafanaIntegrations: {
      azureMonitorWorkspaceIntegrations: [
        {
          azureMonitorWorkspaceResourceId: amw.id
        }
      ]
    }
  }
}

resource raGrafana 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: amw
  name: guid(amw.id, grafana.id, monitoringDataReaderRoleId)
  properties: {
    roleDefinitionId: monitoringDataReaderRoleId
    principalId: grafana.identity.principalId
    principalType: 'ServicePrincipal'
  }
}

output grafanaEndpoint string = grafana.properties.endpoint
output amwId string = amw.id
output amwQueryEndpoint string = amw.properties.metrics.prometheusQueryEndpoint
