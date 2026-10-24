import { ref } from 'vue'
import { defineStore } from 'pinia'
import {
  api,
  type EvaluationState,
  type Machine,
  type Provider,
  type RevisionInfo,
  type Subscription,
} from '../api'

/** Safety net; normal refreshes are pushed by the server over SSE. */
export const FALLBACK_POLL_MS = 60_000
const EVENT_RECONNECT_MS = 5_000
const REFRESH_DEBOUNCE_MS = 150

interface ServerEvent {
  type: 'changed' | 'evaluation'
  scopes?: string[]
  evaluation?: EvaluationState
}

export const usePortalStore = defineStore('portal', () => {
  const subscriptions = ref<Subscription[]>([])
  const providers = ref<Provider[]>([])
  const machines = ref<Machine[]>([])
  const revision = ref<RevisionInfo | null>(null)
  const evaluations = ref<Record<string, EvaluationState>>({})
  const loading = ref(false)
  const live = ref(false)
  const tick = ref(0)
  const dataVersion = ref(0)
  const lastUpdatedAt = ref<number | null>(null)

  let timer: number | null = null
  let reconnectTimer: number | null = null
  let refreshTimer: number | null = null
  let refreshing: Promise<void> | null = null
  let source: EventSource | null = null

  async function loadSubscriptions() {
    subscriptions.value = await api.listSubscriptions()
  }

  async function loadProviders() {
    providers.value = await api.listProviders()
  }

  async function loadMachines() {
    machines.value = await api.listMachines()
    absorbEvaluations()
  }

  async function loadRevision() {
    revision.value = await api.revision()
  }

  function shouldApplyEvaluation(
    current: EvaluationState | undefined,
    incoming: EvaluationState,
  ): boolean {
    if (!current) return true
    if (incoming.startedAt !== current.startedAt) {
      return incoming.startedAt > current.startedAt
    }
    if (incoming.runId !== current.runId) return incoming.runId > current.runId
    if (incoming.finishedAt && !current.finishedAt) return true
    if (!incoming.finishedAt && current.finishedAt) return false

    const rank: Record<string, number> = {
      starting: 0,
      latency: 1,
      bandwidth: 2,
      verify: 3,
      done: 4,
      error: 4,
    }
    const currentRank = rank[current.phase] ?? 0
    const incomingRank = rank[incoming.phase] ?? 0
    if (currentRank !== incomingRank) return incomingRank > currentRank
    return incoming.done >= current.done
  }

  function absorbEvaluations() {
    for (const machine of machines.value) {
      const incoming = machine.evaluation
      if (!incoming) continue
      const current = evaluations.value[machine.name]
      if (shouldApplyEvaluation(current, incoming)) {
        evaluations.value = { ...evaluations.value, [machine.name]: incoming }
      }
      const stored = evaluations.value[machine.name]
      if (stored) machine.evaluation = stored
    }
  }

  async function performRefresh(silent: boolean) {
    if (!silent) loading.value = true
    try {
      await Promise.all([loadSubscriptions(), loadProviders(), loadMachines(), loadRevision()])
      lastUpdatedAt.value = Date.now()
      tick.value += 1
      dataVersion.value += 1
    } finally {
      if (!silent) loading.value = false
    }
  }

  function refreshAll(silent = false) {
    if (refreshing) return refreshing
    refreshing = performRefresh(silent).finally(() => {
      refreshing = null
    })
    return refreshing
  }

  function refreshSilently() {
    return refreshAll(true)
  }

  function scheduleRefresh() {
    if (refreshTimer !== null) return
    refreshTimer = window.setTimeout(() => {
      refreshTimer = null
      refreshSilently().catch(() => {
        // keep the last known state; the fallback poll retries
      })
    }, REFRESH_DEBOUNCE_MS)
  }

  function setEvaluation(evaluation: EvaluationState) {
    const current = evaluations.value[evaluation.machine]
    if (!shouldApplyEvaluation(current, evaluation)) return

    evaluations.value = { ...evaluations.value, [evaluation.machine]: evaluation }
    const machine = machines.value.find((entry) => entry.name === evaluation.machine)
    if (machine) machine.evaluation = evaluation
  }

  function handleEvent(event: MessageEvent<string>) {
    let message: ServerEvent
    try {
      message = JSON.parse(event.data) as ServerEvent
    } catch {
      return
    }

    if (message.type === 'evaluation' && message.evaluation) {
      setEvaluation(message.evaluation)
      return
    }

    scheduleRefresh()
  }

  function connectEvents() {
    if (source || typeof EventSource === 'undefined') return

    source = new EventSource('/api/events')
    source.onopen = () => {
      live.value = true
      scheduleRefresh()
    }
    source.onerror = () => {
      live.value = false
      if (source?.readyState === EventSource.CLOSED) {
        source.close()
        source = null
        if (reconnectTimer === null) {
          reconnectTimer = window.setTimeout(() => {
            reconnectTimer = null
            connectEvents()
          }, EVENT_RECONNECT_MS)
        }
      }
    }
    source.onmessage = handleEvent
  }

  function disconnectEvents() {
    source?.close()
    source = null
    live.value = false
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  }

  function onVisibilityChange() {
    if (!document.hidden) scheduleRefresh()
  }

  function startAutoRefresh() {
    if (timer !== null) return
    connectEvents()
    timer = window.setInterval(() => {
      if (!document.hidden) scheduleRefresh()
    }, FALLBACK_POLL_MS)
    document.addEventListener('visibilitychange', onVisibilityChange)
    refreshAll().catch(() => undefined)
  }

  function stopAutoRefresh() {
    if (timer !== null) {
      window.clearInterval(timer)
      timer = null
    }
    if (refreshTimer !== null) {
      window.clearTimeout(refreshTimer)
      refreshTimer = null
    }
    document.removeEventListener('visibilitychange', onVisibilityChange)
    disconnectEvents()
  }

  function disabledTags(): Set<string> {
    const disabled = new Set<string>()
    for (const provider of providers.value) {
      for (const node of provider.nodes) {
        if (!node.enabled) disabled.add(node.tag)
      }
    }
    return disabled
  }

  function outboundTags(): string[] {
    const disabled = disabledTags()
    const tags = new Set<string>()
    for (const subscription of subscriptions.value) {
      if (!subscription.enabled) continue
      for (const tag of subscription.outboundTags) {
        if (!disabled.has(tag)) tags.add(tag)
      }
    }
    return [...tags].sort()
  }

  function evaluationFor(machine: string | null): EvaluationState | null {
    if (!machine) return null
    return (
      evaluations.value[machine] ??
      machines.value.find((entry) => entry.name === machine)?.evaluation ??
      null
    )
  }

  return {
    subscriptions,
    providers,
    machines,
    revision,
    evaluations,
    loading,
    live,
    tick,
    dataVersion,
    lastUpdatedAt,
    loadSubscriptions,
    loadProviders,
    loadMachines,
    loadRevision,
    refreshAll,
    refreshSilently,
    startAutoRefresh,
    stopAutoRefresh,
    outboundTags,
    evaluationFor,
    setEvaluation,
  }
})
