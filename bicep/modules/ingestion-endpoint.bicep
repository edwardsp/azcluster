param dceName string
param dcrName string
param monUaiPrincipalId string

var metricsPublisherRoleId = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '3913510d-42f4-4e42-8a64-420c390055eb')

resource dce 'Microsoft.Insights/dataCollectionEndpoints@2022-06-01' existing = {
  name: dceName
}

resource dcr 'Microsoft.Insights/dataCollectionRules@2022-06-01' existing = {
  name: dcrName
}

resource raDcr 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: dcr
  name: guid(dcr.id, monUaiPrincipalId, metricsPublisherRoleId)
  properties: {
    roleDefinitionId: metricsPublisherRoleId
    principalId: monUaiPrincipalId
    principalType: 'ServicePrincipal'
  }
}

output ingestionEndpoint string = '${dce.properties.metricsIngestion.endpoint}/dataCollectionRules/${dcr.properties.immutableId}/streams/Microsoft-PrometheusMetrics/api/v1/write?api-version=2023-04-24'
