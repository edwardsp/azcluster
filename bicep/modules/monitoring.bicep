param clusterName string
param location string
param grafanaLocation string
param deployerPrincipalId string = ''
@allowed([
  'User'
  'ServicePrincipal'
  'Group'
])
param deployerPrincipalType string = 'User'
param tags object

var amwName = 'amw-${clusterName}'
var grafanaName = 'amg-${clusterName}'
var monUaiName = 'uai-${clusterName}-mon'
var monitoringDataReaderRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b0d8363b-8ddd-447d-831f-62ca05bff136')
var grafanaAdminRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '22926164-76b3-42b3-bc55-97df8dab3e41')

resource amw 'Microsoft.Monitor/accounts@2023-04-03' = {
  name: amwName
  location: location
  tags: tags
  properties: {
    publicNetworkAccess: 'Enabled'
  }
}

resource monUai 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: monUaiName
  location: location
  tags: tags
}

resource grafana 'Microsoft.Dashboard/grafana@2024-10-01' = {
  name: grafanaName
  location: grafanaLocation
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

resource raDeployerGrafanaAdmin 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(deployerPrincipalId)) {
  scope: grafana
  name: guid(grafana.id, deployerPrincipalId, grafanaAdminRoleId)
  properties: {
    roleDefinitionId: grafanaAdminRoleId
    principalId: deployerPrincipalId
    principalType: deployerPrincipalType
  }
}

var managedRg = 'MA_${amwName}_${location}_managed'

module ingestion 'ingestion-endpoint.bicep' = {
  name: 'ingestionEndpoint'
  scope: resourceGroup(managedRg)
  params: {
    dceName: amwName
    dcrName: amwName
    monUaiPrincipalId: monUai.properties.principalId
  }
  dependsOn: [
    amw
  ]
}

output grafanaEndpoint string = grafana.properties.endpoint
output amwId string = amw.id
output amwQueryEndpoint string = amw.properties.metrics.prometheusQueryEndpoint
output monUaiId string = monUai.id
output monUaiClientId string = monUai.properties.clientId
output monUaiPrincipalId string = monUai.properties.principalId
output ingestionEndpoint string = ingestion.outputs.ingestionEndpoint
output grafanaName string = grafana.name
