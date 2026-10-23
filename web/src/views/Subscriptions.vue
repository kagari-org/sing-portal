<script setup lang="ts">
import { h, onMounted, reactive, ref } from 'vue'
import {
  NButton,
  NCard,
  NDataTable,
  NDrawer,
  NDrawerContent,
  NForm,
  NFormItem,
  NInput,
  NSpace,
  NSwitch,
  NTag,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import { api, formatDuration, formatTimestamp, type Subscription } from '../api'
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()
const message = useMessage()
const busy = ref<string | null>(null)
const drawer = ref(false)
const editing = ref<Subscription | null>(null)

const form = reactive({
  name: '',
  url: '',
  content: '',
  userAgent: '',
  autoRefresh: true,
  refreshInterval: '6h',
})

const columns: DataTableColumns<Subscription> = [
  {
    title: '名称',
    key: 'name',
    render: (row) =>
      h('div', [
        h(NText, { strong: true }, { default: () => row.name }),
        h('div', h(NText, { depth: 3, code: true, style: 'font-size: 12px' }, { default: () => row.slug })),
      ]),
  },
  {
    title: '格式',
    key: 'format',
    render: (row) =>
      h(NTag, { size: 'small' }, { default: () => `${row.format} / ${row.source}` }),
  },
  { title: '节点', key: 'outboundCount', width: 70 },
  {
    title: '启用',
    key: 'enabled',
    width: 70,
    render: (row) =>
      h(NSwitch, {
        value: row.enabled,
        onUpdateValue: (value: boolean) => toggle(row, value),
      }),
  },
  { title: '周期', key: 'refreshIntervalSecs', width: 80, render: (row) => formatDuration(row.refreshIntervalSecs) },
  { title: '上次刷新', key: 'lastFetchedAt', render: (row) => formatTimestamp(row.lastFetchedAt) },
  {
    title: '状态',
    key: 'lastError',
    render: (row) => {
      if (row.lastError) {
        return h(NTag, { type: 'error', size: 'small' }, { default: () => row.lastError })
      }
      return row.validated
        ? h(NTag, { type: 'success', size: 'small' }, { default: () => 'sing-box check 通过' })
        : h(NTag, { size: 'small' }, { default: () => '未校验' })
    },
  },
  {
    title: '操作',
    key: 'actions',
    width: 210,
    render: (row) =>
      h(NSpace, { size: 8 }, {
        default: () => [
          h(
            NButton,
            {
              size: 'small',
              loading: busy.value === row.id,
              onClick: () => refresh(row),
            },
            { default: () => '刷新' },
          ),
          h(NButton, { size: 'small', onClick: () => openEdit(row) }, { default: () => '编辑' }),
          h(
            NButton,
            { size: 'small', type: 'error', ghost: true, onClick: () => remove(row) },
            { default: () => '删除' },
          ),
        ],
      }),
  },
]

function openCreate() {
  editing.value = null
  form.name = ''
  form.url = ''
  form.content = ''
  form.userAgent = ''
  form.autoRefresh = true
  form.refreshInterval = '6h'
  drawer.value = true
}

function openEdit(row: Subscription) {
  editing.value = row
  form.name = row.name
  form.url = ''
  form.content = ''
  form.userAgent = row.userAgent
  form.autoRefresh = row.autoRefresh
  form.refreshInterval = formatDuration(row.refreshIntervalSecs)
  drawer.value = true
}

async function save() {
  if (!form.name.trim()) {
    message.warning('请填写名称')
    return
  }

  try {
    if (editing.value) {
      const mutation = await api.patchSubscription(editing.value.id, {
        name: form.name,
        ...(form.url ? { url: form.url } : {}),
        userAgent: form.userAgent,
        autoRefresh: form.autoRefresh,
        refreshInterval: form.refreshInterval,
      })
      reportWarnings(mutation.warnings)
    } else {
      if (!form.url && !form.content) {
        message.warning('URL 与粘贴内容至少填一个')
        return
      }
      const mutation = await api.createSubscription({
        name: form.name,
        ...(form.url ? { url: form.url } : {}),
        ...(form.content ? { content: form.content } : {}),
        userAgent: form.userAgent,
        autoRefresh: form.autoRefresh,
        refreshInterval: form.refreshInterval,
      })
      reportWarnings(mutation.warnings)
    }
    drawer.value = false
    await Promise.all([store.loadSubscriptions(), store.loadRevision()])
    message.success('已保存')
  } catch (error) {
    message.error((error as Error).message)
  }
}

function reportWarnings(warnings: string[]) {
  if (warnings.length > 0) {
    message.warning(`导入完成，${warnings.length} 条节点被跳过：${warnings[0]}`)
  }
}

async function toggle(row: Subscription, enabled: boolean) {
  try {
    await api.patchSubscription(row.id, { enabled })
    await Promise.all([store.loadSubscriptions(), store.loadRevision()])
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function refresh(row: Subscription) {
  busy.value = row.id
  try {
    const mutation = await api.refreshSubscription(row.id)
    reportWarnings(mutation.warnings)
    await Promise.all([store.loadSubscriptions(), store.loadRevision()])
    message.success('已刷新')
  } catch (error) {
    message.error((error as Error).message)
    await store.loadSubscriptions()
  } finally {
    busy.value = null
  }
}

async function remove(row: Subscription) {
  try {
    await api.deleteSubscription(row.id)
    await Promise.all([store.loadSubscriptions(), store.loadRevision()])
    message.success('已删除')
  } catch (error) {
    message.error((error as Error).message)
  }
}

onMounted(() => {
  store.loadSubscriptions().catch(() => undefined)
})
</script>

<template>
  <div class="toolbar">
    <h2 style="margin: 0">订阅</h2>
    <n-button type="primary" @click="openCreate">导入订阅</n-button>
  </div>

  <n-card>
    <n-data-table :columns="columns" :data="store.subscriptions" :bordered="false" />
  </n-card>

  <n-drawer v-model:show="drawer" :width="520">
    <n-drawer-content :title="editing ? `编辑 ${editing.name}` : '导入订阅'">
      <n-form label-placement="top">
        <n-form-item label="名称">
          <n-input v-model:value="form.name" placeholder="例如 机场A" />
        </n-form-item>
        <n-form-item label="订阅 URL">
          <n-input
            v-model:value="form.url"
            :placeholder="editing ? '留空表示不修改' : 'https://example.com/sub?token=…'"
          />
        </n-form-item>
        <n-form-item v-if="!editing" label="或粘贴订阅内容">
          <n-input
            v-model:value="form.content"
            type="textarea"
            :rows="8"
            placeholder="sing-box JSON / Clash YAML / base64 或明文 URI 列表"
          />
        </n-form-item>
        <n-form-item label="User-Agent">
          <n-input v-model:value="form.userAgent" placeholder="留空使用 sing-box/1.14.0" />
        </n-form-item>
        <n-form-item label="自动刷新">
          <n-space align="center">
            <n-switch v-model:value="form.autoRefresh" />
            <n-input v-model:value="form.refreshInterval" style="width: 100px" placeholder="6h" />
          </n-space>
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="drawer = false">取消</n-button>
          <n-button type="primary" @click="save">保存</n-button>
        </n-space>
      </template>
    </n-drawer-content>
  </n-drawer>
</template>
