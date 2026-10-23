<script setup lang="ts">
import { computed, h, onMounted, ref } from 'vue'
import {
  NButton,
  NCard,
  NDataTable,
  NSelect,
  NSpace,
  NTag,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import { api, formatLatency, formatTimestamp, type Machine } from '../api'
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()
const message = useMessage()
const busy = ref<string | null>(null)
const latency = ref<Record<string, number | null>>({})
const selectionOptions = computed(() => [
  { label: 's_auto (auto select)', value: 's_auto' },
  { label: 's_direct (direct)', value: 's_direct' },
  ...store.outboundTags().map((tag) => ({ label: tag, value: tag })),
])

const columns = computed<DataTableColumns<Machine>>(() => [
  { title: 'Machine', key: 'name' },
  {
    title: 'Clash API',
    key: 'address',
    render: (row) => h(NText, { code: true }, { default: () => row.address }),
  },
  {
    title: 'Reachable',
    key: 'reachable',
    width: 110,
    render: (row) =>
      h(NTag, { type: row.reachable ? 'success' : 'error', size: 'small' }, {
        default: () => (row.reachable ? row.version ?? 'online' : 'unreachable'),
      }),
  },
  {
    title: 'Egress',
    key: 'selection',
    width: 260,
    render: (row) =>
      h(NSelect, {
        value: row.selection,
        options: selectionOptions.value,
        size: 'small',
        filterable: true,
        onUpdateValue: (value: string) => choose(row, value),
      }),
  },
  {
    title: 'Latency',
    key: 'latency',
    width: 110,
    render: (row) => formatLatency(latency.value[row.name] ?? null),
  },
  {
    title: 'Reported revision',
    key: 'reportedRevision',
    render: (row) =>
      h(NText, { depth: 3, title: row.report?.revision || '' }, {
        default: () => shortRevision(row.report?.revision),
      }),
  },
  {
    title: 'Health',
    key: 'health',
    width: 190,
    render: (row) =>
      h(NTag, { size: 'small', type: healthType(row.report?.health ?? '') }, {
        default: () => row.report?.health || 'no report',
      }),
  },
  {
    title: 'Last report',
    key: 'reportedAt',
    render: (row) => formatTimestamp(row.report?.reportedAt ?? null),
  },
  {
    title: 'Actions',
    key: 'actions',
    width: 130,
    render: (row) =>
      h(
        NButton,
        {
          size: 'small',
          loading: busy.value === row.name,
          onClick: () => test(row),
        },
        { default: () => 'Test latency' },
      ),
  },
])

function shortRevision(revision: string | null | undefined): string {
  if (!revision) return '—'
  if (revision.length <= 18) return revision
  return `${revision.slice(0, 18)}…`
}

function healthType(health: string): 'success' | 'warning' | 'error' | 'default' {
  if (health === 'ok') return 'success'
  if (health === 'no_active_subscriptions') return 'warning'
  if (health === '') return 'default'
  return 'error'
}

async function choose(machine: Machine, value: string) {
  try {
    const result = await api.setSelection(machine.name, value)
    await store.loadMachines()
    if (result.applied) {
      message.success(`${machine.name} switched to ${result.selection}`)
    } else {
      message.warning(`${machine.name} is unreachable, desired egress queued: ${result.selection}`)
    }
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function test(machine: Machine) {
  busy.value = machine.name
  try {
    const result = await api.runTest(machine.name, machine.selection, 'latency')
    latency.value[machine.name] = result.latencyMs
    message.success(`${machine.selection}: ${formatLatency(result.latencyMs)} via ${result.source}`)
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    busy.value = null
  }
}

onMounted(() => {
  Promise.all([store.loadSubscriptions(), store.loadMachines()]).catch(() => undefined)
})
</script>

<template>
  <div class="toolbar">
    <h2 style="margin: 0">Machines</h2>
    <n-space>
      <n-button @click="store.loadMachines()">Refresh</n-button>
    </n-space>
  </div>

  <n-card>
    <n-data-table :columns="columns" :data="store.machines" :bordered="false" />
  </n-card>
</template>
