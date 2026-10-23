const WRITE_HEADER = 'X-Portal-Request'

export interface Subscription {
  id: string
  name: string
  slug: string
  source: 'url' | 'manual'
  url: string | null
  userAgent: string
  format: string
  enabled: boolean
  autoRefresh: boolean
  refreshIntervalSecs: number
  nextRefreshAt: number | null
  lastFetchedAt: number | null
  lastError: string
  validated: boolean
  failures: number
  createdAt: number
  updatedAt: number
  outboundCount: number
  outboundTags: string[]
}

export interface Mutation {
  subscription: Subscription
  warnings: string[]
}

export interface Report {
  revision: string
  selection: string
  health: string
  error: string
  reportedAt: number
  online: boolean
}

export interface Machine {
  name: string
  address: string
  selection: string
  mode: 'auto' | 'manual'
  score: number | null
  best: string | null
  bestScore: number | null
  measuredAt: number | null
  reachable: boolean
  version: string | null
  report: Report | null
}

export interface MeasurementRow {
  tag: string
  latencyMs: number | null
  downMbps: number | null
  score: number
  measuredAt: number
  best: boolean
}

export interface ProviderNode {
  tag: string
  kind: string
  server: string
  serverPort: number | null
}

export interface Provider {
  id: string
  name: string
  slug: string
  format: string
  enabled: boolean
  lastError: string
  lastFetchedAt: number | null
  nodes: ProviderNode[]
}

export type TestKind = 'latency' | 'bandwidth' | 'both'

export interface Measurement {
  tag: string
  kind: TestKind
  latencyMs: number | null
  downMbps: number | null
  upMbps: number | null
  responsiveness: number | null
  source: 'agent'
}

export interface RevisionInfo {
  revision: string
  outbounds: number
  enabledSubscriptions: number
}

export interface SelectionResult {
  selection: string
  applied: boolean
}

export interface AuditEntry {
  id: number
  at: number
  actor: string
  action: string
  target: string
  detail: string
}

export interface Snapshot {
  version: number
  exportedAt: number
  subscriptions: Subscription[]
  selections: { nodeName: string; outbound: string; updatedAt: number }[]
}

export interface SubscriptionInput {
  name: string
  url?: string
  content?: string
  userAgent?: string
  enabled?: boolean
  autoRefresh?: boolean
  refreshInterval?: string
}

export interface SubscriptionPatchInput {
  name?: string
  url?: string
  userAgent?: string
  enabled?: boolean
  autoRefresh?: boolean
  refreshInterval?: string
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const method = (init.method ?? 'GET').toUpperCase()
  const headers: Record<string, string> = { ...(init.headers as Record<string, string> | undefined) }
  if (method !== 'GET') {
    headers[WRITE_HEADER] = '1'
  }
  if (init.body !== undefined) {
    headers['Content-Type'] = 'application/json'
  }

  const response = await fetch(path, { ...init, headers })
  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`
    try {
      const data = (await response.json()) as { error?: string }
      if (data?.error) message = data.error
    } catch {
      // keep the status line
    }
    throw new Error(message)
  }
  if (response.status === 204) {
    return undefined as T
  }
  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

export const api = {
  listSubscriptions: () => request<Subscription[]>('/api/subscriptions'),
  createSubscription: (input: SubscriptionInput) =>
    request<Mutation>('/api/subscriptions', { method: 'POST', body: JSON.stringify(input) }),
  patchSubscription: (id: string, input: SubscriptionPatchInput) =>
    request<Mutation>(`/api/subscriptions/${id}`, { method: 'PATCH', body: JSON.stringify(input) }),
  deleteSubscription: (id: string) =>
    request<void>(`/api/subscriptions/${id}`, { method: 'DELETE' }),
  refreshSubscription: (id: string) =>
    request<Mutation>(`/api/subscriptions/${id}/refresh`, { method: 'POST' }),
  revision: () => request<RevisionInfo>('/api/revision'),
  listProviders: () => request<Provider[]>('/api/providers'),
  listMachines: () => request<Machine[]>('/api/nodes'),
  listMeasurements: (machine: string) =>
    request<MeasurementRow[]>(`/api/nodes/${machine}/measurements`),
  setMode: (machine: string, mode: 'auto' | 'manual') =>
    request<{ mode: string }>(`/api/nodes/${machine}/mode`, {
      method: 'PUT',
      body: JSON.stringify({ mode }),
    }),
  runAuto: (machine: string) =>
    request<{ status: string }>(`/api/nodes/${machine}/auto`, { method: 'POST' }),
  setSelection: (machine: string, outbound: string) =>
    request<SelectionResult>(`/api/nodes/${machine}/selection`, {
      method: 'PUT',
      body: JSON.stringify({ outbound }),
    }),
  runTest: (machine: string, tag: string, kind: TestKind) =>
    request<Measurement>(`/api/nodes/${machine}/test`, {
      method: 'POST',
      body: JSON.stringify({ tag, kind }),
    }),
  listAudit: (limit = 100) => request<AuditEntry[]>(`/api/audit?limit=${limit}`),
  exportSnapshot: () => request<Snapshot>('/api/export'),
  importSnapshot: (snapshot: Snapshot) =>
    request<void>('/api/import', { method: 'POST', body: JSON.stringify(snapshot) }),
}

export function formatTimestamp(value: number | null): string {
  if (!value) return '—'
  return new Date(value * 1000).toLocaleString()
}

export function formatDuration(seconds: number): string {
  if (seconds % 86400 === 0) return `${seconds / 86400}d`
  if (seconds % 3600 === 0) return `${seconds / 3600}h`
  if (seconds % 60 === 0) return `${seconds / 60}m`
  return `${seconds}s`
}

export function formatMbps(value: number | null): string {
  return value === null ? '—' : `${value.toFixed(1)} Mbps`
}

export function formatLatency(value: number | null): string {
  if (value === null) return '—'
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ms`
}
