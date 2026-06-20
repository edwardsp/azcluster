param clusterName string
param location string
param aksClusterName string
param amwId string
param tags object

var dcName = take('MSProm-${location}-${clusterName}', 44)

resource aks 'Microsoft.ContainerService/managedClusters@2025-10-01' existing = {
  name: aksClusterName
}

resource dce 'Microsoft.Insights/dataCollectionEndpoints@2023-03-11' = {
  name: dcName
  location: location
  kind: 'Linux'
  tags: tags
  properties: {}
}

resource dcr 'Microsoft.Insights/dataCollectionRules@2023-03-11' = {
  name: dcName
  location: location
  kind: 'Linux'
  tags: tags
  properties: {
    dataCollectionEndpointId: dce.id
    dataSources: {
      prometheusForwarder: [
        {
          name: 'PrometheusDataSource'
          streams: [
            'Microsoft-PrometheusMetrics'
          ]
          labelIncludeFilter: {}
        }
      ]
    }
    destinations: {
      monitoringAccounts: [
        {
          accountResourceId: amwId
          name: 'MonitoringAccount1'
        }
      ]
    }
    dataFlows: [
      {
        streams: [
          'Microsoft-PrometheusMetrics'
        ]
        destinations: [
          'MonitoringAccount1'
        ]
      }
    ]
    description: 'Managed Prometheus DCR for azcluster AKS -> AMW'
  }
}

resource dcra 'Microsoft.Insights/dataCollectionRuleAssociations@2023-03-11' = {
  name: 'ContainerInsightsMetricsExtension'
  scope: aks
  properties: {
    dataCollectionRuleId: dcr.id
    description: 'Associates the managed-Prometheus DCR with the AKS cluster.'
  }
}
