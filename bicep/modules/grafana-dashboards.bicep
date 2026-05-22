param grafanaName string

resource grafana 'Microsoft.Dashboard/grafana@2024-10-01' existing = {
  name: grafanaName
}

resource nodeDashboard 'Microsoft.Dashboard/grafana/dashboards@2024-10-01' = {
  parent: grafana
  name: 'azcluster-node-health'
  properties: {
    grafanaDashboard: loadJsonContent('../../grafana/dashboards/node.json')
  }
}

resource slurmDashboard 'Microsoft.Dashboard/grafana/dashboards@2024-10-01' = {
  parent: grafana
  name: 'azcluster-slurm-scheduler'
  properties: {
    grafanaDashboard: loadJsonContent('../../grafana/dashboards/slurm.json')
  }
}

resource gpuIbDashboard 'Microsoft.Dashboard/grafana/dashboards@2024-10-01' = {
  parent: grafana
  name: 'azcluster-gpu-ib'
  properties: {
    grafanaDashboard: loadJsonContent('../../grafana/dashboards/gpu_ib.json')
  }
}

output nodeDashboardId string = nodeDashboard.id
output slurmDashboardId string = slurmDashboard.id
output gpuIbDashboardId string = gpuIbDashboard.id
