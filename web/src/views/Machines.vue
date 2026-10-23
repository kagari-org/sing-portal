<script setup lang="ts">
import { computed, h, onMounted, ref } from 'vue'
import {
  NButton,
  NCard,
  NDataTable,
  NSelect,
  NSpace,
  NSwitch,
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
  { label: 's_auto (fallback)', value: 's_auto' },
  { label: 's_direct (direct)', value: 's_direct' },
  ...store.outboundTags().map((tag) => ({ label: tag, value: tag })),
])

function shortRevision(revision: string | null | undefined): string {
  if (!revision) return '—'
  if (revision.length <= 18) return revision
  return `${revision.slice(0, 18)}…`
}

function formatScore(score: number | null | undefined): string {
  if (score === null || score === undefined) return '—'
  return score.toFixed(1)
}

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
    title: 'Auto',
    key: 'mode',
    width: 80,
    render: (row) =>
      h(NSwitch, {
        value: row.mode === 'auto',
        onUpdateValue: (value: boolean) => {
          if (value !== (row.mode === 'auto')) setMode(row, value)
        },
      }),
  },
  {
    title: 'Egress',
    key: 'selection',
    width: 240,
    render: (row) =>
      h(NSelect, {
        value: row.selection,
        options: selectionOptions.value,
        size: 'small',
        filterable: true,
        disabled: row.mode === 'auto',
        onUpdateValue: (value: string) => choose(row, value),
      }),
  },
  {
    title: 'Score',
    key: 'score',
    width: 90,
    render: (row) => formatScore(row.score),
  },
  {
    title: 'Best',
    key: 'best',
    render: (row) =>
      h(NText, { depth: 3 }, {
        default: () => (row.best ? `${row.best} (${formatScore(row.bestScore)})` : '—'),
      }),
  },
  {
    title: 'Latency',
    key: 'latency',
    width: 100,
    render: (row) => formatLatency(latency.value[row.name] ?? null),
  },
  {
    title: 'Health',
    key: 'health',
    width: 170,
    render: (row) =>
      h(NTag, { size: 'small', type: healthType(row.report?.health ?? '') }, {
        default: () => row.report?.health || 'no report',
      }),
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
    title: 'Last report',
    key: 'reportedAt',
    render: (row) => formatTimestamp(row.report?.reportedAt ?? null),
  },
  {
    title: 'Actions',
    key: 'actions',
    width: 200,
    render: (row) =>
      h(NSpace, { size: 8 }, {
        default: () => [
          h(
            NButton,
            {
              size: 'small',
              disabled: row.mode !== 'auto' || busy.value === `auto:${row.name}`,
              loading: busy.value === `auto:${row.name}`,
              onClick: () => evaluate(row),
            },
            { default: () => 'Evaluate' },
          ),
          h(
            NButton,
            {
              size: 'small',
              loading: busy.value === `latency:${row.name}`,
              disabled: testTag(row) === null,
              onClick: () => test(row),
            },
            { default: () => 'Test latency' },
          ),
        ],
      }),
  },
])

function healthType(health: string): 'success' | 'warning' | 'error' | 'default' {
  if (health === 'ok') return 'success'
  if (health === 'no_active_subscriptions') return 'warning'
  if (health === '') return 'default'
  return 'error'
}

async function setMode(machine: Machine, auto: boolean) {
  if (auto === (machine.mode === 'auto')) return
  try {
    await api.setMode(machine.name, auto ? 'auto' : 'manual')
    message.success(`${machine.name}: ${auto ? 'auto' : 'manual'} selection`)
    setTimeout(() => store.loadMachines().catch(() => undefined), auto ? 1500 : 0)
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function evaluate(machine: Machine) {
  busy.value = `auto:${machine.name}`
  try {
    const result = await api.runAuto(machine.name)
    message.info(`evaluation ${result.status}`)
    setTimeout(() => store.loadMachines().catch(() => undefined), 2000)
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    busy.value = null
  }
}

async function choose(machine: Machine, value: string) {
  const current = store.machines.find((entry) => entry.name === machine.name)
  if (!value || !current || current.mode === 'auto' || value === current.selection) return
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

function testTag(machine: Machine): string | null {
  const tags = new Set(selectionOptions.value.map((option) => option.value))
  if (tags.has(machine.selection)) return machine.selection
  if (machine.best && tags.has(machine.best)) return machine.best
  return null
}

async function test(machine: Machine) {
  const tag = testTag(machine)
  if (!tag) {
    message.warning('automatic selection has not picked a node yet')
    return
  }
  busy.value = `latency:${machine.name}`
  try {
    const result = await api.runTest(machine.name, tag, 'latency')
    latency.value[machine.name] = result.latencyMs
    message.success(`${tag}: ${formatLatency(result.latencyMs)} via ${result.source}`)
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
    <n-data-table :columns="columns" :data="store.machines" :bordered="false" :scroll-x="1400" />
  </n-card>
</template>
