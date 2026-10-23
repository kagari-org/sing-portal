<script setup lang="ts">
import { h, onMounted, ref } from 'vue'
import {
  NAlert,
  NButton,
  NCard,
  NDataTable,
  NSpace,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import { api, formatTimestamp, type AuditEntry, type Snapshot } from '../api'

const message = useMessage()
const audit = ref<AuditEntry[]>([])
const importing = ref(false)
const fileInput = ref<HTMLInputElement | null>(null)

const auditColumns: DataTableColumns<AuditEntry> = [
  { title: '时间', key: 'at', render: (row) => formatTimestamp(row.at) },
  { title: '来源', key: 'actor', width: 130 },
  { title: '动作', key: 'action', width: 180 },
  { title: '目标', key: 'target', width: 180 },
  { title: '详情', key: 'detail' },
]

async function loadAudit() {
  try {
    audit.value = await api.listAudit(50)
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function download() {
  try {
    const snapshot = await api.exportSnapshot()
    const blob = new Blob([JSON.stringify(snapshot, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `sing-portal-export-${Date.now()}.json`
    anchor.click()
    URL.revokeObjectURL(url)
    message.success('已导出（包含订阅凭据，请妥善保管）')
  } catch (error) {
    message.error((error as Error).message)
  }
}

function pickFile() {
  fileInput.value?.click()
}

async function onFilePicked(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  importing.value = true
  try {
    const snapshot = JSON.parse(await file.text()) as Snapshot
    await api.importSnapshot(snapshot)
    message.success('导入完成')
    await loadAudit()
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    importing.value = false
    input.value = ''
  }
}

onMounted(loadAudit)
</script>

<template>
  <n-space vertical size="large">
    <n-alert type="warning" title="备份包含订阅凭据">
      导出文件里有订阅原文和 URL，等同于控制面全部权限，请按密钥对待。
    </n-alert>

    <n-card title="备份与恢复">
      <n-space>
        <n-button type="primary" @click="download">导出 JSON</n-button>
        <n-button :loading="importing" @click="pickFile">导入 JSON</n-button>
        <input ref="fileInput" type="file" accept="application/json" hidden @change="onFilePicked" />
      </n-space>
      <p>
        <n-text depth="3">导入会整体替换现有订阅与出口选择，操作记录写入审计日志。</n-text>
      </p>
    </n-card>

    <n-card title="审计日志">
      <template #header-extra>
        <n-button size="small" @click="loadAudit">刷新</n-button>
      </template>
      <n-data-table
        :columns="auditColumns"
        :data="audit"
        :bordered="false"
        :max-height="420"
        virtual-scroll
      />
    </n-card>
  </n-space>
</template>
