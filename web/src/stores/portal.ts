import { ref } from 'vue'
import { defineStore } from 'pinia'
import {
  api,
  type Machine,
  type Provider,
  type RevisionInfo,
  type Subscription,
} from '../api'

export const AUTO_REFRESH_MS = 5000

export const usePortalStore = defineStore('portal', () => {
  const subscriptions = ref<Subscription[]>([])
  const providers = ref<Provider[]>([])
  const machines = ref<Machine[]>([])
  const revision = ref<RevisionInfo | null>(null)
  const loading = ref(false)
  const tick = ref(0)
  const lastUpdatedAt = ref<number | null>(null)

  let timer: number | null = null
  let inFlight = false

  async function loadSubscriptions() {
    subscriptions.value = await api.listSubscriptions()
  }

  async function loadProviders() {
    providers.value = await api.listProviders()
  }

  async function loadMachines() {
    machines.value = await api.listMachines()
  }

  async function loadRevision() {
    revision.value = await api.revision()
  }

  async function refreshAll() {
    loading.value = true
    try {
      await Promise.all([loadSubscriptions(), loadProviders(), loadMachines(), loadRevision()])
      lastUpdatedAt.value = Date.now()
      tick.value += 1
    } finally {
      loading.value = false
    }
  }

  async function poll() {
    if (typeof document !== 'undefined' && document.hidden) return
    if (inFlight || loading.value) return
    inFlight = true
    try {
      await refreshAll()
    } catch {
      // keep the last known state; the next tick retries
    } finally {
      inFlight = false
    }
  }

  function onVisibilityChange() {
    if (!document.hidden) poll()
  }

  function startAutoRefresh() {
    if (timer !== null) return
    timer = window.setInterval(poll, AUTO_REFRESH_MS)
    document.addEventListener('visibilitychange', onVisibilityChange)
    poll()
  }

  function stopAutoRefresh() {
    if (timer !== null) {
      window.clearInterval(timer)
      timer = null
    }
    document.removeEventListener('visibilitychange', onVisibilityChange)
  }

  function outboundTags(): string[] {
    const tags = new Set<string>()
    for (const subscription of subscriptions.value) {
      if (!subscription.enabled) continue
      for (const tag of subscription.outboundTags) tags.add(tag)
    }
    return [...tags].sort()
  }

  return {
    subscriptions,
    providers,
    machines,
    revision,
    loading,
    tick,
    lastUpdatedAt,
    loadSubscriptions,
    loadProviders,
    loadMachines,
    loadRevision,
    refreshAll,
    startAutoRefresh,
    stopAutoRefresh,
    outboundTags,
  }
})
