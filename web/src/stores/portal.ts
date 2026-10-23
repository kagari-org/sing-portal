import { ref } from 'vue'
import { defineStore } from 'pinia'
import {
  api,
  type Machine,
  type Provider,
  type RevisionInfo,
  type Subscription,
} from '../api'

export const usePortalStore = defineStore('portal', () => {
  const subscriptions = ref<Subscription[]>([])
  const providers = ref<Provider[]>([])
  const machines = ref<Machine[]>([])
  const revision = ref<RevisionInfo | null>(null)
  const loading = ref(false)

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
    } finally {
      loading.value = false
    }
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
    loadSubscriptions,
    loadProviders,
    loadMachines,
    loadRevision,
    refreshAll,
    outboundTags,
  }
})
