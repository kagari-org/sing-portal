<script setup lang="ts">
import { computed, h, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  NAlert,
  NButton,
  NCard,
  NDataTable,
  NProgress,
  NSelect,
  NSpace,
  NSwitch,
  NTag,
  NText,
  NTooltip,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import {
  api,
  formatLatency,
  formatMbps,
  formatTimestamp,
  type ManualMeasurementRow,
  type Measurement,
  type MeasurementRow,
  type Provider,
  type ProviderNode,
  type TestKind,
} from '../api'
import { usePortalStore } from '../stores/portal'

interface Row extends ProviderNode {
  providerName: string
  providerSlug: string
  providerEnabled: boolean
}

const route = useRoute()
const router = useRouter()
const store = usePortalStore()
const message = useMessage()

const testMachine = ref<string | null>(null)
const autoMeasurements = ref<MeasurementRow[]>([])
const manualMeasurements = reactive<Record<string, ManualMeasurementRow>>({})
const enabledOverride = reactive<Record<string, boolean>>({})
const errors = reactive<Record<string, string>>({})
const nodeBusy = reactive<Record<string, boolean>>({})
const busy = ref<string | null>(null)
const bulk = ref<TestKind | null>(null)
const progress = ref({ done: 0, total: 0 })
const requesting = ref(false)
const modeBusy = ref(false)
const now = ref(Math.floor(Date.now() / 1000))
let clock: number | null = null

const autoByTag = computed(() => new Map(autoMeasurements.value.map((row) => [row.tag, row])))

const currentMachine = computed(
  () => store.machines.find((machine) => machine.name === testMachine.value) ?? null,
)
const machineMode = computed(() => currentMachine.value?.mode ?? null)
const machineSelection = computed(() => currentMachine.value?.selection ?? null)
const egressDegraded = computed(
  () => currentMachine.value?.report?.health === 'egress_degraded',
)
const evaluation = computed(() => store.evaluationFor(testMachine.value))

const machineOptions = computed(() =>
  store.machines.map((machine) => ({
    label: `${machine.name} ${machine.reachable ? '' : '(unreachable)'}`.trim(),
    value: machine.name,
  })),
)

const selectionOptions = computed(() => {
  const options = [
    { label: 's_auto (urltest)', value: 's_auto' },
    { label: 's_direct (direct)', value: 's_direct' },
    ...store.outboundTags().map((tag) => ({ label: tag, value: tag })),
  ]
  const current = machineSelection.value
  if (current && !options.some((option) => option.value === current)) {
    options.push({ label: `${current} (unavailable)`, value: current })
  }
  return options
})

const providers = computed<Provider[]>(() =>
  [...store.providers].sort((left, right) => {
    if (left.enabled !== right.enabled) return left.enabled ? -1 : 1
    return left.name.localeCompare(right.name)
  }),
)

const phaseLabel = computed(() => {
  switch (evaluation.value?.phase) {
    case 'starting':
      return 'starting'
    case 'latency':
      return 'probing latency'
    case 'bandwidth':
      return 'testing bandwidth'
    case 'verify':
      return 'verifying egress'
    case 'done':
      return 'done'
    case 'error':
      return 'failed'
    default:
      return evaluation.value?.phase ?? ''
  }
})

const progressPercent = computed(() => {
  const current = evaluation.value
  if (!current) return 0
  if (!current.running) return 100
  if (current.phase === 'starting') return 2

  const ratio = current.total > 0 ? Math.min(1, current.done / current.total) : 0
  const [start, end] =
    current.phase === 'latency'
      ? [0, 40]
      : current.phase === 'bandwidth'
        ? [40, 90]
        : current.phase === 'verify'
          ? [90, 100]
          : [0, 100]
  return Math.round(start + (end - start) * ratio)
})

const elapsed = computed(() => {
  const current = evaluation.value
  if (!current?.startedAt) return ''
  const end = current.running ? now.value : (current.finishedAt ?? now.value)
  const seconds = Math.max(0, end - current.startedAt)
  if (seconds < 60) return `${seconds}s`
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`
})

const columns: DataTableColumns<Row> = [
  {
    title: 'Node',
    key: 'tag',
    render: (row) =>
      h(NSpace, { size: 6, align: 'center' }, {
        default: () => [
          h(NText, { code: true }, { default: () => row.tag }),
          row.providerEnabled
            ? null
            : h(NTag, { size: 'small', type: 'warning' }, { default: () => 'subscription off' }),
          row.enabled
            ? null
            : h(NTag, { size: 'small', type: 'error' }, { default: () => 'disabled' }),
        ],
      }),
  },
  { title: 'Type', key: 'kind', width: 110 },
  {
    title: 'Server',
    key: 'server',
    render: (row) => `${row.server}:${row.serverPort ?? ''}`,
  },
  {
    title: 'Eval',
    key: 'eval',
    width: 190,
    render: (row) => {
      const result = autoByTag.value.get(row.tag)
      if (!result) return h(NText, { depth: 3 }, { default: () => '—' })
      if (result.latencyMs === null && result.downMbps === null) {
        return h(NText, { type: 'error' }, { default: () => 'failed' })
      }
      return h(
        NTooltip,
        {},
        {
          trigger: () =>
            h('div', { style: 'line-height: 1.35' }, [
              h('div', formatLatency(result.latencyMs)),
              result.downMbps === null
                ? null
                : h(NText, { depth: 3, style: 'font-size: 12px' }, {
                    default: () => formatMbps(result.downMbps),
                  }),
            ]),
          default: () => `Automatic measurement ${formatTimestamp(result.measuredAt)}`,
        },
      )
    },
  },
  {
    title: 'Score',
    key: 'score',
    width: 120,
    render: (row) => {
      const result = autoByTag.value.get(row.tag)
      if (!result || result.downMbps === null) {
        return h(NText, { depth: 3 }, { default: () => '—' })
      }
      return h(NSpace, { size: 6, align: 'center' }, {
        default: () => [
          h(NText, { depth: 2 }, { default: () => result.score.toFixed(1) }),
          result.best
            ? h(NTag, { size: 'small', type: 'success' }, { default: () => 'best' })
            : null,
        ],
      })
    },
  },
  {
    title: 'Manual',
    key: 'manual',
    width: 190,
    render: (row) => {
      const result = manualMeasurements[row.tag]
      if (!result) {
        const error = errors[row.tag]
        if (error) {
          return h(NText, { type: 'error', style: 'font-size: 12px' }, { default: () => error })
        }
        return h(NText, { depth: 3 }, { default: () => '—' })
      }
      return h(
        NTooltip,
        {},
        {
          trigger: () =>
            h('div', { style: 'line-height: 1.35' }, [
              h('div', formatLatency(result.latencyMs)),
              h(NText, { depth: 3, style: 'font-size: 12px' }, {
                default: () => formatMbps(result.downMbps),
              }),
            ]),
          default: () => `Manual measurement ${formatTimestamp(result.measuredAt)}`,
        },
      )
    },
  },
  {
    title: 'Enabled',
    key: 'enabled',
    width: 90,
    render: (row) =>
      h(NSwitch, {
        size: 'small',
        value: row.enabled,
        loading: nodeBusy[row.tag] === true,
        onUpdateValue: (value: boolean) => toggleNode(row, value),
      }),
  },
  {
    title: 'Actions',
    key: 'actions',
    width: 190,
    render: (row) =>
      h(NSpace, { size: 8 }, {
        default: () => [
          testButton(row, 'latency', 'Latency'),
          testButton(row, 'bandwidth', 'Bandwidth'),
        ],
      }),
  },
]

function providerRows(provider: Provider): Row[] {
  return provider.nodes.map((node) => ({
    ...node,
    enabled: enabledOverride[node.tag] ?? node.enabled,
    providerName: provider.name,
    providerSlug: provider.slug,
    providerEnabled: provider.enabled,
  }))
}

function keyOf(row: Row): string {
  return row.tag
}

function testButton(row: Row, kind: TestKind, label: string) {
  const key = `${keyOf(row)}|${kind}`
  return h(
    NButton,
    {
      size: 'small',
      loading: busy.value === key,
      disabled:
        !testMachine.value ||
        !row.enabled ||
        !row.providerEnabled ||
        bulk.value !== null ||
        evaluation.value?.running === true,
      onClick: () => runTest(row, kind),
    },
    { default: () => label },
  )
}

function remember(row: Row, kind: TestKind, result: Measurement) {
  const tag = keyOf(row)
  delete errors[tag]
  const previous = manualMeasurements[tag]
  const latencyMs =
    result.latencyMs ?? (kind === 'bandwidth' ? (previous?.latencyMs ?? null) : null)
  const downMbps =
    result.downMbps ?? (kind === 'latency' ? (previous?.downMbps ?? null) : null)
  manualMeasurements[tag] = {
    tag: row.tag,
    latencyMs,
    downMbps,
    measuredAt: Math.floor(Date.now() / 1000),
  }
}

async function runTest(row: Row, kind: TestKind) {
  const machine = testMachine.value
  if (!machine) {
    message.warning('Pick a machine to run the test on')
    return
  }
  if (!row.enabled || !row.providerEnabled) {
    message.warning('This proxy is disabled')
    return
  }
  const key = `${keyOf(row)}|${kind}`
  busy.value = key
  try {
    const result = await api.runTest(machine, row.tag, kind)
    if (machine === testMachine.value) {
      remember(row, kind, result)
      if (kind === 'latency') {
        message.success(`${row.tag}: ${formatLatency(result.latencyMs)}`)
      } else {
        message.success(`${row.tag}: ${formatMbps(result.downMbps)}`)
      }
    }
  } catch (error) {
    if (machine === testMachine.value) {
      errors[keyOf(row)] = (error as Error).message
      message.error((error as Error).message)
    }
  } finally {
    busy.value = null
  }
}

async function runAll(kind: TestKind) {
  const machine = testMachine.value
  if (!machine) {
    message.warning('Pick a machine to run the tests on')
    return
  }
  if (evaluation.value?.running) {
    message.warning('Wait for the running evaluation to finish')
    return
  }

  const targets = providers.value
    .filter((provider) => provider.enabled)
    .flatMap((provider) => providerRows(provider))
    .filter((row) => row.enabled)
  if (!targets.length) {
    message.warning('No enabled proxies to test')
    return
  }

  bulk.value = kind
  progress.value = { done: 0, total: targets.length }
  let failed = 0

  // latency probes are cheap and the agent runs them concurrently; network
  // quality tests are serialised there, so only send one at a time
  const limit = kind === 'latency' ? 8 : 1
  let next = 0

  const worker = async () => {
    while (machine === testMachine.value) {
      const index = next++
      if (index >= targets.length) return
      const row = targets[index]
      try {
        const result = await api.runTest(machine, row.tag, kind)
        if (machine !== testMachine.value) return
        remember(row, kind, result)
      } catch (error) {
        if (machine !== testMachine.value) return
        failed += 1
        errors[keyOf(row)] = (error as Error).message
      }
      progress.value = { done: progress.value.done + 1, total: targets.length }
    }
  }

  try {
    await Promise.all(Array.from({ length: Math.min(limit, targets.length) }, worker))
  } finally {
    bulk.value = null
  }

  if (failed) {
    message.warning(`${kind} test finished: ${targets.length - failed} ok, ${failed} failed`)
  } else {
    message.success(`${kind} test finished for ${targets.length} proxies`)
  }
}

async function evaluate() {
  const machine = currentMachine.value
  if (!machine) {
    message.warning('Pick a machine to evaluate on')
    return
  }
  if (machine.mode !== 'auto') {
    message.warning('Switch the machine to auto before evaluating')
    return
  }

  requesting.value = true
  try {
    const result = await api.runAuto(machine.name)
    message.info(
      result.status === 'started' ? 'Evaluation started' : 'Evaluation is already running',
    )
    const state = await api.getEvaluation(machine.name)
    if (state) store.setEvaluation(state)
    await store.loadMachines()
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    requesting.value = false
  }
}

async function toggleAuto(value: boolean) {
  const machine = currentMachine.value
  if (!machine || value === (machine.mode === 'auto')) return
  modeBusy.value = true
  try {
    await api.setMode(machine.name, value ? 'auto' : 'manual')
    await store.refreshSilently()
    message.success(`${machine.name}: ${value ? 'auto' : 'manual'} selection`)
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    modeBusy.value = false
  }
}

async function chooseEgress(value: string) {
  const machine = currentMachine.value
  if (!machine || !value || machine.mode !== 'manual' || value === machine.selection) return
  try {
    const result = await api.setSelection(machine.name, value)
    await store.refreshSilently()
    if (result.applied) {
      message.success(`${machine.name} switched to ${result.selection}`)
    } else {
      message.warning(`${machine.name} is unreachable, desired egress queued: ${result.selection}`)
    }
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function toggleNode(row: Row, enabled: boolean) {
  const tag = row.tag
  enabledOverride[tag] = enabled
  nodeBusy[tag] = true
  try {
    await api.setNodeEnabled(tag, enabled)
    await store.refreshSilently()
    await loadMeasurements()
    message.success(`${tag} ${enabled ? 'enabled' : 'disabled'}`)
  } catch (error) {
    delete enabledOverride[tag]
    message.error((error as Error).message)
  } finally {
    delete nodeBusy[tag]
    delete enabledOverride[tag]
  }
}

async function loadMeasurements() {
  const machine = testMachine.value
  if (!machine) {
    autoMeasurements.value = []
    for (const tag of Object.keys(manualMeasurements)) delete manualMeasurements[tag]
    return
  }

  try {
    const result = await api.listMeasurements(machine)
    if (machine !== testMachine.value) return
    autoMeasurements.value = result.auto

    const next: Record<string, ManualMeasurementRow> = {}
    for (const row of result.manual) next[row.tag] = row
    for (const [tag, local] of Object.entries(manualMeasurements)) {
      const stored = next[tag]
      if (!stored || local.measuredAt > stored.measuredAt) next[tag] = local
    }
    for (const tag of Object.keys(manualMeasurements)) delete manualMeasurements[tag]
    Object.assign(manualMeasurements, next)
  } catch {
    // keep the last known results; the next update retries
  }
}

async function reload() {
  try {
    await store.refreshSilently()
    await loadMeasurements()
  } catch (error) {
    message.error((error as Error).message)
  }
}

function queryMachine(): string | null {
  const value = route.query.machine
  return typeof value === 'string' && value ? value : null
}

function resolveMachine(): string | null {
  const fromQuery = queryMachine()
  if (fromQuery && store.machines.some((machine) => machine.name === fromQuery)) {
    return fromQuery
  }
  return store.machines.find((machine) => machine.reachable)?.name ?? store.machines[0]?.name ?? null
}

function rememberMachine(name: string | null) {
  if (queryMachine() === name) return
  const query = { ...route.query }
  if (name) query.machine = name
  else delete query.machine
  router.replace({ query })
}

function startClock() {
  if (clock !== null) return
  now.value = Math.floor(Date.now() / 1000)
  clock = window.setInterval(() => {
    now.value = Math.floor(Date.now() / 1000)
  }, 1000)
}

function stopClock() {
  if (clock !== null) {
    window.clearInterval(clock)
    clock = null
  }
}

watch(
  () => evaluation.value?.running,
  (running) => {
    if (running) startClock()
    else stopClock()
  },
  { immediate: true },
)

watch(testMachine, (value, previous) => {
  rememberMachine(value)
  if (value !== previous) {
    autoMeasurements.value = []
    for (const tag of Object.keys(manualMeasurements)) delete manualMeasurements[tag]
    for (const tag of Object.keys(errors)) delete errors[tag]
  }
  loadMeasurements()
})

watch(
  () => route.query.machine,
  (value) => {
    if (typeof value === 'string' && value && value !== testMachine.value) {
      testMachine.value = value
    }
  },
)

watch(
  () => store.dataVersion,
  () => {
    loadMeasurements()
  },
)

onMounted(async () => {
  await Promise.all([store.loadProviders(), store.loadMachines()])
  const target = resolveMachine()
  testMachine.value = target
  if (target) rememberMachine(target)
  await loadMeasurements()
})

onUnmounted(stopClock)
</script>

<template>
  <div class="toolbar">
    <h2 style="margin: 0">Proxies</h2>
    <n-space align="center" :size="8">
      <n-text depth="3">Run tests on</n-text>
      <n-select
        v-model:value="testMachine"
        :options="machineOptions"
        size="small"
        style="width: 200px"
        placeholder="no machines"
      />
      <n-text depth="3">Auto</n-text>
      <n-switch
        :value="machineMode === 'auto'"
        :loading="modeBusy"
        :disabled="!currentMachine || evaluation?.running"
        @update:value="toggleAuto"
      />
      <n-text depth="3">Egress</n-text>
      <n-select
        :value="machineSelection"
        :options="selectionOptions"
        size="small"
        filterable
        style="width: 260px"
        :disabled="machineMode !== 'manual' || !currentMachine || evaluation?.running"
        placeholder="manual selection"
        @update:value="chooseEgress"
      />
      <n-button
        size="small"
        type="primary"
        :loading="requesting || evaluation?.running"
        :disabled="
          machineMode !== 'auto' ||
          evaluation?.running ||
          bulk !== null ||
          busy !== null
        "
        :title="machineMode === 'manual' ? 'switch the machine to auto first' : undefined"
        @click="evaluate"
      >
        {{ evaluation?.running ? 'Evaluating…' : 'Evaluate' }}
      </n-button>
      <n-button
        size="small"
        :loading="bulk === 'latency'"
        :disabled="bulk !== null || busy !== null || evaluation?.running"
        @click="runAll('latency')"
      >
        Test all latency
      </n-button>
      <n-button
        size="small"
        :loading="bulk === 'bandwidth'"
        :disabled="bulk !== null || busy !== null || evaluation?.running"
        @click="runAll('bandwidth')"
      >
        Test all bandwidth
      </n-button>
      <n-text v-if="bulk" depth="3">
        {{ progress.done }}/{{ progress.total }}
      </n-text>
      <n-button size="small" @click="reload">Reload</n-button>
    </n-space>
  </div>

  <n-alert
    v-if="evaluation?.running"
    type="info"
    :bordered="false"
    style="margin-bottom: 16px"
  >
    <n-space align="center" :size="12">
      <n-progress
        type="line"
        :percentage="progressPercent"
        :show-indicator="false"
        :height="8"
        style="width: 200px"
      />
      <n-text strong>{{ phaseLabel }}</n-text>
      <n-text depth="2">{{ evaluation.done }}/{{ evaluation.total }}</n-text>
      <n-text
        v-if="evaluation.current"
        code
        style="max-width: 360px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap"
      >
        {{ evaluation.current }}
      </n-text>
      <n-text depth="3">elapsed {{ elapsed }}</n-text>
    </n-space>
  </n-alert>

  <n-alert
    v-else-if="evaluation?.finishedAt"
    :type="evaluation.error ? 'warning' : evaluation.switched ? 'success' : 'default'"
    :bordered="false"
    style="margin-bottom: 16px"
  >
    <n-space align="center" :size="8">
      <n-text>
        Last evaluation {{ formatTimestamp(evaluation.finishedAt) }}
        <template v-if="evaluation.error"> · {{ evaluation.error }}</template>
        <template v-else-if="evaluation.best">
          · picked <n-text code>{{ evaluation.best }}</n-text>
          ({{ evaluation.score?.toFixed(1) }})
          · {{ evaluation.switched ? 'switched' : 'kept current' }}
          · checked {{ evaluation.checked }} candidate(s)
        </template>
      </n-text>
    </n-space>
  </n-alert>

  <n-alert
    v-if="egressDegraded && !evaluation?.running"
    type="warning"
    :bordered="false"
    style="margin-bottom: 16px"
  >
    The real egress health check is failing on {{ currentMachine?.name }}.
    <template v-if="machineMode === 'auto'">
      A recovery evaluation will start automatically.
    </template>
    <template v-else>Switch to auto or choose another egress.</template>
  </n-alert>

  <n-space vertical size="large">
    <n-card v-for="provider in providers" :key="provider.id" :bordered="true">
      <template #header>
        <n-space align="center" size="small">
          <n-text strong>{{ provider.name }}</n-text>
          <n-tag size="small">{{ provider.format }}</n-tag>
          <n-tag v-if="!provider.enabled" size="small" type="warning">disabled</n-tag>
          <n-text depth="3">{{ provider.nodes.length }} nodes</n-text>
        </n-space>
      </template>
      <template #header-extra>
        <n-text v-if="provider.lastError" type="error">
          {{ provider.lastError }}
        </n-text>
      </template>
      <n-data-table
        :columns="columns"
        :data="providerRows(provider)"
        :bordered="false"
        :max-height="420"
        size="small"
      />
    </n-card>

    <n-card v-if="!providers.length">
      <n-text depth="3">No subscriptions yet. Import one on the Subscriptions page.</n-text>
    </n-card>
  </n-space>
</template>
