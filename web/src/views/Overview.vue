<script setup lang="ts">
import { onMounted } from 'vue'
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
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()

onMounted(() => {
  store.refreshAll().catch(() => undefined)
})

function healthType(health: string): 'success' | 'warning' | 'error' | 'default' {
  if (health === 'ok') return 'success'
  if (health === 'no_active_subscriptions') return 'warning'
  if (health === '') return 'default'
  return 'error'
}
</script>

<template>
  <n-space vertical size="large">
    <n-alert v-if="!store.machines.length" type="warning" title="No machines configured">
      Start the portal with <n-text code>--nodes "home=10.11.0.3:9090"</n-text>.
    </n-alert>

    <n-grid cols="1 640:3" :x-gap="16" :y-gap="16">
      <n-grid-item>
        <n-card title="Outbound revision">
          <n-statistic :value="store.revision?.revision ?? '—'" />
          <n-text depth="3">
            {{ store.revision?.outbounds ?? 0 }} nodes · {{ store.revision?.enabledSubscriptions ?? 0 }}
            active subscriptions
          </n-text>
        </n-card>
      </n-grid-item>
      <n-grid-item>
        <n-card title="Subscriptions">
          <n-statistic :value="String(store.subscriptions.length)" />
          <n-text depth="3">
            {{ store.subscriptions.filter((item) => item.enabled).length }} active ·
            {{ store.subscriptions.filter((item) => item.lastError).length }} failing
          </n-text>
        </n-card>
      </n-grid-item>
      <n-grid-item>
        <n-card title="Machines">
          <n-statistic :value="String(store.machines.filter((item) => item.reachable).length)" />
          <n-text depth="3">Clash API reachable out of {{ store.machines.length }}</n-text>
        </n-card>
      </n-grid-item>
    </n-grid>

    <n-card title="Machine status">
      <n-table :bordered="false" :single-line="false">
        <thead>
          <tr>
            <th>Machine</th>
            <th>Address</th>
            <th>Reachable</th>
            <th>Egress</th>
            <th>Reported revision</th>
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
            <td><n-text depth="3">{{ machine.report?.revision || '—' }}</n-text></td>
            <td>
              <n-tag :type="healthType(machine.report?.health ?? '')" size="small">
                {{ machine.report?.health || 'no report' }}
              </n-tag>
            </td>
            <td>{{ formatTimestamp(machine.report?.reportedAt ?? null) }}</td>
          </tr>
        </tbody>
      </n-table>
    </n-card>
  </n-space>
</template>
