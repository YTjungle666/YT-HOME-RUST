<template>
  <LogVue v-model="logModal.visible" :control="logModal" :visible="logModal.visible" />
  <Backup v-model="backupModal.visible" :control="backupModal" :visible="backupModal.visible" />
  <UsageStats v-model:visible="usageStatsModal.visible" />
  <v-container class="fill-height" :loading="loading">
    <v-responsive :class="reloadItems.length>0 ? 'fill-height text-center' : 'align-center'" >
      <v-row class="d-flex align-center justify-center">
        <v-col cols="auto">
          <v-img src="@/assets/logo.svg" :width="reloadItems.length>0 ? 100 : 200"></v-img>
        </v-col>
      </v-row>
      <v-row class="d-flex align-center justify-center">
        <v-col cols="auto">
          <v-dialog v-model="menu" :close-on-content-click="false" transition="scale-transition" max-width="800">
            <template v-slot:activator="{ props }">
              <v-btn v-bind="props" hide-details variant="tonal" elevation="3">{{ $t('main.tiles') }} <v-icon icon="mdi-star-plus" /></v-btn>
            </template>
            <v-card rounded="xl">
              <v-card-title>
                <v-row>
                  <v-col>
                    {{ $t('main.tiles') }}
                  </v-col>
                  <v-spacer></v-spacer>
                  <v-col cols="auto"><v-icon icon="mdi-close" @click="menu = false"></v-icon></v-col>
                </v-row>
              </v-card-title>
              <v-divider></v-divider>
              <v-row v-for="items in menuItems" :key="items.title" density="compact">
                <v-col cols="12">
                  <v-card :subtitle="items.title" variant="flat">
                    <v-card-text>
                      <v-row density="compact">
                        <v-col cols="12" md="6" lg="3" v-for="item in items.value" :key="item.value">
                          <v-switch
                          density="compact"
                          v-model="reloadItems"
                          :value="item.value"
                          color="primary"
                          :label="item.title"
                          hide-details></v-switch>
                        </v-col>
                      </v-row>
                    </v-card-text>
                  </v-card>
                </v-col>
              </v-row>
            </v-card>
          </v-dialog>
          <v-btn variant="tonal" hide-details
            style="margin-inline-start: 10px;" elevation="3"
            @click="backupModal.visible = true">{{ $t('main.backup.title') }}<v-icon icon="mdi-backup-restore" />
          </v-btn>
          <v-btn variant="tonal" hide-details
            style="margin-inline-start: 10px;" elevation="3"
            @click="logModal.visible = true">{{ $t('basic.log.title') }} <v-icon icon="mdi-list-box-outline" />
          </v-btn>
          <v-btn variant="tonal" hide-details
            style="margin-inline-start: 10px;" elevation="3"
            @click="usageStatsModal.visible = true">{{ $t('main.stats.title') }} <v-icon icon="mdi-chart-box-outline" />
          </v-btn>
        </v-col>
      </v-row>
      <v-row>
        <v-col cols="12" sm="6" md="3" v-for="i in reloadItems" :key="i">
          <v-card class="rounded-lg" variant="outlined" height="210px" elevation="5">
            <v-card-title>
              {{ menuItems.flatMap(cat => cat.value).find(m => m.value == i)?.title }}
              <template v-if="i == 'i-sys'">
                <v-icon icon="mdi-update" color="primary"
                  @click="reloadSys()" size="small" v-tooltip:top="$t('actions.update')"
                  style="margin-inline-start: 10px;">
                </v-icon>
              </template>
              <template v-if="i == 'h-net'">
                <v-icon icon="mdi-information" color="primary" size="small"
                  v-tooltip:top="'↓' +
                  HumanReadable.sizeFormat(tilesData.net?.recv) + ' - ' +
                  HumanReadable.sizeFormat(tilesData.net?.sent) + '↑'"
                  style="margin-inline-start: 10px;">
                </v-icon>
              </template>
            </v-card-title>
            <v-card-text style="padding: 0 16px;" align="center" justify="center">
              <Gauge :tilesData="tilesData" :type="i" v-if="i.charAt(0) == 'g'" />
              <History :tilesData="tilesData" :type="i" v-if="i.charAt(0) == 'h'" />
              <template v-if="i == 'i-sys'">
                <v-row>
                  <v-col cols="3">{{ $t('main.info.host') }}</v-col>
                  <v-col cols="9" style="text-wrap: nowrap; overflow: hidden">{{ tilesData.sys?.hostName }}</v-col>
                  <v-col cols="3">{{ $t('main.info.cpu') }}</v-col>
                  <v-col cols="9">
                    <v-chip density="compact" variant="flat">
                      <v-tooltip activator="parent" location="top" style="direction: ltr;">
                        {{ tilesData.sys?.cpuType }}
                      </v-tooltip>
                     {{ tilesData.sys?.cpuCount }} {{ $t('main.info.core') }}
                    </v-chip>
                  </v-col>
                  <v-col cols="3">IP</v-col>
                  <v-col cols="9">
                    <v-chip density="compact" color="primary" variant="flat" v-if="tilesData.sys?.ipv4?.length>0">
                      <v-tooltip activator="parent" location="top" style="direction: ltr;">
                        <span v-html="tilesData.sys?.ipv4?.join('<br />')"></span>
                      </v-tooltip>
                      IPv4
                    </v-chip>
                    <v-chip density="compact" color="primary" variant="flat" v-if="tilesData.sys?.ipv6?.length>0">
                      <v-tooltip activator="parent" location="top" style="direction: ltr;">
                        <span v-html="tilesData.sys?.ipv6?.join('<br />')"></span>
                      </v-tooltip>
                      IPv6
                    </v-chip>
                  </v-col>
                  <v-col cols="3">YT HOME</v-col>
                  <v-col cols="9">
                    <v-chip density="compact" color="blue">
                      v{{ tilesData.sys?.appVersion }}
                    </v-chip>
                  </v-col>
                  <v-col cols="3">{{ $t('main.info.uptime') }}</v-col>
                  <v-col cols="9" v-tooltip:top="$t('main.info.startupTime')
                    + ': ' + systemStartupTimeText">
                    {{ systemUptimeText }}
                  </v-col>
                </v-row>
              </template>
              <template v-if="i == 'i-sbd'">
                <v-row>
                  <v-col cols="4">{{ $t('main.info.running') }}</v-col>
                  <v-col cols="8">
                    <v-chip density="compact" color="success" variant="flat" v-if="tilesData.sbd?.running">{{ $t('yes') }}</v-chip>
                    <v-chip density="compact" color="error" variant="flat" v-else>{{ $t('no') }}</v-chip>
                    <v-chip density="compact" color="transparent" v-if="tilesData.sbd?.running && !loading" style="cursor: pointer;" @click="restartSingbox()">
                      <v-tooltip activator="parent" location="top">
                        {{ $t('actions.restartSb') }}
                      </v-tooltip>
                      <v-icon icon="mdi-restart" color="warning" />
                    </v-chip>
                  </v-col>
                  <v-col cols="4">{{ $t('main.info.memory') }}</v-col>
                  <v-col cols="8">
                    <v-chip density="compact" color="primary" variant="flat" v-if="tilesData.sbd?.stats?.Alloc">
                      {{ HumanReadable.sizeFormat(tilesData.sbd?.stats?.Alloc) }}
                    </v-chip>
                  </v-col>
                  <v-col cols="4">{{ $t('main.info.threads') }}</v-col>
                  <v-col cols="8">
                    <v-chip density="compact" color="primary" variant="flat" v-if="tilesData.sbd?.stats?.NumGoroutine">
                      {{ tilesData.sbd?.stats?.NumGoroutine }}
                    </v-chip>
                  </v-col>
                  <v-col cols="4">{{ $t('main.info.uptime') }}</v-col>
                  <v-col cols="8">{{ coreUptimeText }}</v-col>
                  <v-col cols="4">{{ $t('online') }}</v-col>
                  <v-col cols="8">
                    <template v-if="tilesData.sbd?.running">
                      <v-chip density="compact" color="primary" variant="flat" v-if="Data().onlines.user">
                        <v-tooltip activator="parent" location="top" overflow="auto">
                          <span v-text="$t('pages.clients')" style="font-weight: bold;"></span><br/>
                          <span v-for="user in Data().onlines.user" :key="user">{{ user }}<br /></span>
                        </v-tooltip>
                        {{ Data().onlines.user?.length }}
                      </v-chip>
                      <v-chip density="compact" color="success" variant="flat" v-if="Data().onlines.inbound">
                        <v-tooltip activator="parent" location="top" :text="$t('pages.inbounds')">
                          <span v-text="$t('pages.inbounds')" style="font-weight: bold;"></span><br/>
                          <span v-for="i in Data().onlines.inbound" :key="i">{{ i }}<br /></span>
                        </v-tooltip>
                        {{ Data().onlines.inbound?.length }}
                      </v-chip>
                      <v-chip density="compact" color="info" variant="flat" v-if="Data().onlines.outbound">
                        <v-tooltip activator="parent" location="top" :text="$t('pages.outbounds')">
                          <span v-text="$t('pages.outbounds')" style="font-weight: bold;"></span><br/>
                          <span v-for="o in Data().onlines.outbound" :key="o">{{ o }}<br /></span>
                        </v-tooltip>
                        {{ Data().onlines.outbound?.length }}
                      </v-chip>
                    </template>
                  </v-col>
                </v-row>
              </template>
            </v-card-text>
          </v-card>
        </v-col>
      </v-row>
    </v-responsive>
  </v-container>
</template>

<script lang="ts" setup>
import HttpUtils from '@/plugins/httputil'
import { HumanReadable } from '@/plugins/utils'
import Data from '@/store/modules/data'
import Gauge from '@/components/tiles/Gauge.vue'
import History from '@/components/tiles/History.vue'
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { i18n, locale } from '@/locales'
import LogVue from '@/layouts/modals/Logs.vue'
import Backup from '@/layouts/modals/Backup.vue'
import UsageStats from '@/layouts/modals/UsageStats.vue'

const loading = ref(false)
const menu = ref(false)
const menuItems = [
  { title: i18n.global.t('main.gauges'), value: [
    { title: i18n.global.t('main.gauge.cpu'), value: "g-cpu" },
    { title: i18n.global.t('main.gauge.mem'), value: "g-mem" },
    { title: i18n.global.t('main.gauge.dsk'), value: "g-dsk" },
    { title: i18n.global.t('main.gauge.swp'), value: "g-swp" },
    ]
  },
  { title: i18n.global.t('main.charts'), value: [
    { title: i18n.global.t('main.chart.cpu'), value: "h-cpu" },
    { title: i18n.global.t('main.chart.mem'), value: "h-mem" },
    { title: i18n.global.t('main.chart.net'), value: "h-net" },
    { title: i18n.global.t('main.chart.pnet'), value: "hp-net" },
    { title: i18n.global.t('main.chart.dio'), value: "h-dio" },
    ]
  },
  { title: i18n.global.t('main.infos'), value: [
    { title: i18n.global.t('main.info.sys'), value: "i-sys" },
    { title: i18n.global.t('main.info.sbd'), value: "i-sbd" },
    ]
  },
]

const tilesData = ref(<any>{})
const nowSeconds = ref(Math.floor(Date.now() / 1000))
const coreStatusFetchedAt = ref(0)

const reloadItems = computed({
  get() { return Data().reloadItems },
  set(v:string[]) {
    if (Data().reloadItems.length == 0 && v.length>0) startTimer()
    if (Data().reloadItems.length > 0 && v.length == 0) stopTimer()
    Data().reloadItems = v
    if (v.length > 0) {
      localStorage.setItem("reloadItems", v.join(','))
    } else {
      localStorage.removeItem("reloadItems")
    }
  }
})

const mergeTilesData = (payload: any) => {
  if (payload?.sbd?.stats?.Uptime !== undefined) {
    coreStatusFetchedAt.value = nowSeconds.value
  }
  tilesData.value = {
    ...tilesData.value,
    ...payload,
  }
}

const systemUptimeSeconds = computed(() => {
  const bootTime = Number(tilesData.value?.sys?.bootTime ?? 0)
  if (!bootTime) return 0
  return Math.max(0, nowSeconds.value - bootTime)
})

const systemStartupTimeText = computed(() => {
  const bootTime = Number(tilesData.value?.sys?.bootTime ?? 0)
  if (!bootTime) return '-'
  return new Date(bootTime * 1000).toLocaleString(locale)
})

const systemUptimeText = computed(() => HumanReadable.formatSecond(systemUptimeSeconds.value))

const coreUptimeSeconds = computed(() => {
  const base = Number(tilesData.value?.sbd?.stats?.Uptime ?? 0)
  if (!tilesData.value?.sbd?.running || coreStatusFetchedAt.value === 0) {
    return base
  }
  return base + Math.max(0, nowSeconds.value - coreStatusFetchedAt.value)
})

const coreUptimeText = computed(() => HumanReadable.formatSecond(coreUptimeSeconds.value))

const reloadData = async () => {
  let request = [...new Set(reloadItems.value.map(r => r.split('-')[1]))]
  if (tilesData.value?.sys?.appVersion) {
    request = request.filter(r => r != 'sys')
  }
  if (request.length === 0) return
  const data = await HttpUtils.get('api/status',{ r: request.join(',')})
  if (data.success) {
    mergeTilesData(data.obj)
  }
}

const reloadSys = async () => {
  const data = await HttpUtils.get('api/status',{ r: 'sys'})
  if (data.success) {
    mergeTilesData(data.obj)
  }
}

let intervalId: ReturnType<typeof setInterval> | null = null
let clockIntervalId: ReturnType<typeof setInterval> | null = null

const startTimer = () => {
  intervalId = setInterval(() => {
    reloadData()
  }, 2000)
}

const startClock = () => {
  clockIntervalId = setInterval(() => {
    nowSeconds.value = Math.floor(Date.now() / 1000)
  }, 1000)
}

const stopTimer = () => {
  if (intervalId) {
    clearInterval(intervalId)
    intervalId = null
  }
}

const stopClock = () => {
  if (clockIntervalId) {
    clearInterval(clockIntervalId)
    clockIntervalId = null
  }
}

onMounted(async () => {
  loading.value = true
  startClock()
  if (Data().reloadItems.length != 0) {
    await reloadData()
    startTimer()
  }
  loading.value = false
})

onBeforeUnmount(() => {
  stopTimer()
  stopClock()
})

const logModal = ref({ visible: false })

const backupModal = ref({ visible: false })

const usageStatsModal = ref({ visible: false })

const restartSingbox = async () => {
  loading.value = true
  await HttpUtils.post('api/restartSb',{})
  loading.value = false
}
</script>
