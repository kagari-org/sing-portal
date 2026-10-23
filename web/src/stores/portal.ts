import { ref } from 'vue'
import { defineStore } from 'pinia'
import { api, type NodeInfo, type RevisionInfo, type Subscription } from '../api'

export const usePortalStore = defineStore('portal', () => {
  const subscriptions = ref<Subscription[]>([])
  const nodes = ref<NodeInfo[]>([])
  const revision = ref<RevisionInfo | null>(null)
  const loading = ref(false)

  async function loadSubscriptions() {
    subscriptions.value = await api.listSubscriptions()
  }

  async function loadNodes() {
    nodes.value = await api.listNodes()
  }

  async function loadRevision() {
    revision.value = await api.revision()
  }

  async function refreshAll() {
    loading.value = true
    try {
      await Promise.all([loadSubscriptions(), loadNodes(), loadRevision()])
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
    nodes,
    revision,
    loading,
    loadSubscriptions,
    loadNodes,
    loadRevision,
    refreshAll,
    outboundTags,
  }
})
