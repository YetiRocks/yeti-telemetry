import { useState, useEffect, useCallback } from 'react'
import { NavId, Connector, ConnectorConfig } from './types'
import { Sidebar } from './components/Sidebar'
import { LogsPanel } from './components/LogsPanel'
import { SpansPanel } from './components/SpansPanel'
import { MetricsPanel } from './components/MetricsPanel'
import { SettingsPanel } from './components/SettingsPanel'
import { ConnectorDetail } from './components/ConnectorDetail'
import { AddConnectorModal } from './components/AddConnectorModal'

const VALID_NAV: NavId[] = ['logs', 'spans', 'metrics', 'status']

function getNavFromHash(): NavId {
  const hash = window.location.hash.replace('#/', '').replace('#', '')
  return VALID_NAV.includes(hash as NavId) ? (hash as NavId) : 'logs'
}

function App() {
  const [selected, setSelected] = useState<NavId>(getNavFromHash)
  const [paused, setPaused] = useState(false)
  const [connectors, setConnectors] = useState<Connector[]>([])
  const [selectedConnector, setSelectedConnector] = useState<string | null>(null)
  const [showAddConnector, setShowAddConnector] = useState(false)

  // Sync hash → state on popstate (back/forward)
  useEffect(() => {
    const onHashChange = () => {
      setSelected(getNavFromHash())
      setSelectedConnector(null)
    }
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [])

  // Sync state → hash on nav change
  const handleSelect = useCallback((id: NavId) => {
    setSelected(id)
    window.location.hash = `#/${id}`
  }, [])

  const connections = { logs: true, spans: true, metrics: true }

  const handleSelectConnector = useCallback((id: string) => {
    if (id === '') {
      setSelectedConnector(null)
    } else {
      setSelectedConnector(id)
      setSelected('connector' as NavId)
    }
  }, [])

  const handleAddConnector = useCallback((connector: Connector) => {
    setConnectors(prev => [...prev, connector])
    setSelectedConnector(connector.id)
    setSelected('connector' as NavId)
  }, [])

  const handleToggleConnector = useCallback((id: string) => {
    setConnectors(prev => prev.map(c => c.id === id ? { ...c, enabled: !c.enabled } : c))
  }, [])

  const handleRemoveConnector = useCallback((id: string) => {
    setConnectors(prev => prev.filter(c => c.id !== id))
    setSelectedConnector(null)
    handleSelect('logs')
  }, [handleSelect])

  const handleUpdateConfig = useCallback((id: string, config: ConnectorConfig) => {
    setConnectors(prev => prev.map(c => c.id === id ? { ...c, config } : c))
  }, [])

  const activeConnector = connectors.find(c => c.id === selectedConnector) || null

  return (
    <div className="app">
      <header className="header">
        <div className="header-left">
          <img src="./logo_white.svg" alt="Yeti" className="logo" />
        </div>
        <div className="header-title">Telemetry</div>
      </header>

      <div className="main">
        <Sidebar
          selected={selected}
          onSelect={handleSelect}
          connectors={connectors}
          selectedConnector={selectedConnector}
          onSelectConnector={handleSelectConnector}
          onAddConnector={() => setShowAddConnector(true)}
        />

        <div className="content">
          <div className="panel-container" style={{ display: selected === 'logs' && !selectedConnector ? 'flex' : 'none' }}>
            <LogsPanel paused={paused} onTogglePause={() => setPaused(p => !p)} />
          </div>
          <div className="panel-container" style={{ display: selected === 'spans' && !selectedConnector ? 'flex' : 'none' }}>
            <SpansPanel paused={paused} onTogglePause={() => setPaused(p => !p)} />
          </div>
          <div className="panel-container" style={{ display: selected === 'metrics' && !selectedConnector ? 'flex' : 'none' }}>
            <MetricsPanel paused={paused} onTogglePause={() => setPaused(p => !p)} />
          </div>
          <div className="panel-container" style={{ display: selected === 'status' && !selectedConnector ? 'flex' : 'none' }}>
            <SettingsPanel connections={connections} />
          </div>
          {selectedConnector && activeConnector && (
            <ConnectorDetail
              connector={activeConnector}
              onToggle={handleToggleConnector}
              onRemove={handleRemoveConnector}
              onUpdateConfig={handleUpdateConfig}
            />
          )}
        </div>
      </div>

      {showAddConnector && (
        <AddConnectorModal
          onClose={() => setShowAddConnector(false)}
          onAdd={handleAddConnector}
        />
      )}
    </div>
  )
}

export default App
