<script setup lang="ts">
import { computed, onMounted } from 'vue'
import {
  NAlert,
  NCard,
  NGrid,
  NGridItem,
  NStatistic,
  NSpace,
  NTable,
  NTag,
  NText,
} from 'naive-ui'
import { formatTimestamp } from '../api'
import { useIsCompact } from '../composables/useMediaQuery'
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()
const isCompact = useIsCompact()

onMounted(() => {
  store.refreshAll().catch(() => undefined)
})

const proxyCount = computed(() =>
  store.providers
    .filter((provider) => provider.enabled)
    .reduce((total, provider) => total + provider.nodes.length, 0),
)

const activeProviders = computed(() => store.providers.filter((provider) => provider.enabled).length)

function healthType(health: string): 'success' | 'warning' | 'error' | 'default' {
  if (health === 'ok') return 'success'
  if (health === 'no_active_subscriptions') return 'warning'
  if (health === '') return 'default'
  return 'error'
}

function shortRevision(revision: string | null | undefined): string {
  if (!revision) return '—'
  if (revision.length <= 18) return revision
  return `${revision.slice(0, 18)}…`
}
</script>

<template>
  <div class="toolbar">
    <h2 class="page-title">Overview</h2>
  </div>

  <n-space vertical size="large">
    <n-alert v-if="!store.machines.length" type="warning" title="No machines configured">
      Start the portal with <n-text code>--nodes "home=10.11.0.3:9090"</n-text>.
    </n-alert>

    <n-grid cols="1 640:3" :x-gap="16" :y-gap="16">
      <n-grid-item>
        <n-card title="Proxies">
          <n-statistic :value="proxyCount" />
          <n-text depth="3">
            {{ activeProviders }} of {{ store.providers.length }} subscriptions active
          </n-text>
        </n-card>
      </n-grid-item>
      <n-grid-item>
        <n-card title="Subscriptions">
          <n-statistic :value="store.subscriptions.length" />
          <n-text depth="3">
            {{ store.subscriptions.filter((item) => item.enabled).length }} active ·
            {{ store.subscriptions.filter((item) => item.lastError).length }} failing
          </n-text>
        </n-card>
      </n-grid-item>
      <n-grid-item>
        <n-card title="Machines">
          <n-statistic :value="store.machines.filter((item) => item.reachable).length" />
          <n-text depth="3">Clash API reachable out of {{ store.machines.length }}</n-text>
        </n-card>
      </n-grid-item>
    </n-grid>

    <n-card title="Machine status">
      <n-table v-if="!isCompact" :bordered="false" :single-line="false">
        <thead>
          <tr>
            <th>Machine</th>
            <th>Address</th>
            <th>Reachable</th>
            <th>Egress</th>
            <th>Revision</th>
            <th>Health</th>
            <th>Last report</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="machine in store.machines" :key="machine.name">
            <td>{{ machine.name }}</td>
            <td><n-text code>{{ machine.address }}</n-text></td>
            <td>
              <n-tag :type="machine.reachable ? 'success' : 'error'" size="small">
                {{ machine.reachable ? 'online' : 'unreachable' }}
              </n-tag>
            </td>
            <td>{{ machine.selection }}</td>
            <td>
              <n-text depth="3" :title="machine.report?.revision || ''">
                {{ shortRevision(machine.report?.revision) }}
              </n-text>
            </td>
            <td>
              <n-tag :type="healthType(machine.report?.health ?? '')" size="small">
                {{ machine.report?.health || 'no report' }}
              </n-tag>
            </td>
            <td>{{ formatTimestamp(machine.report?.reportedAt ?? null) }}</td>
          </tr>
        </tbody>
      </n-table>

      <div v-else class="machine-list">
        <div v-for="machine in store.machines" :key="machine.name" class="machine-item">
          <div class="item-head">
            <div class="item-heading">
              <n-text strong>{{ machine.name }}</n-text>
              <n-text code class="text-break">{{ machine.address }}</n-text>
            </div>
            <n-tag :type="machine.reachable ? 'success' : 'error'" size="small">
              {{ machine.reachable ? 'online' : 'unreachable' }}
            </n-tag>
          </div>
          <div class="field-grid">
            <div class="field">
              <n-text depth="3" class="field-label">Egress</n-text>
              <n-text class="text-break">{{ machine.selection }}</n-text>
            </div>
            <div class="field">
              <n-text depth="3" class="field-label">Health</n-text>
              <n-tag :type="healthType(machine.report?.health ?? '')" size="small">
                {{ machine.report?.health || 'no report' }}
              </n-tag>
            </div>
            <div class="field">
              <n-text depth="3" class="field-label">Revision</n-text>
              <n-text depth="3" class="text-break" :title="machine.report?.revision || ''">
                {{ shortRevision(machine.report?.revision) }}
              </n-text>
            </div>
            <div class="field">
              <n-text depth="3" class="field-label">Last report</n-text>
              <n-text>{{ formatTimestamp(machine.report?.reportedAt ?? null) }}</n-text>
            </div>
          </div>
        </div>
        <n-text v-if="!store.machines.length" depth="3">No machines connected yet.</n-text>
      </div>
    </n-card>
  </n-space>
</template>

<style scoped>
.machine-item + .machine-item {
  margin-top: 14px;
  padding-top: 14px;
  border-top: 1px solid rgba(128, 128, 128, 0.2);
}
</style>
