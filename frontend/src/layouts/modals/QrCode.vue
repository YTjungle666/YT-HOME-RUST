<template>
  <v-dialog transition="dialog-bottom-transition" width="400">
    <v-card class="rounded-lg" id="qrcode-modal" :loading="loading">
      <v-card-title>
        <v-row>
          <v-col>QrCode</v-col>
          <v-spacer></v-spacer>
          <v-col cols="auto"><v-icon icon="mdi-close-box" @click="$emit('close')" /></v-col>
        </v-row>
      </v-card-title>
      <v-divider></v-divider>
      <v-skeleton-loader
          class="mx-auto border"
          width="80%"
          type="text, image, divider, text, image"
          v-if="loading"
        ></v-skeleton-loader>
      <v-card-text style="overflow-y: auto; padding: 0" :hidden="loading">
        <v-tabs
          v-model="tab"
          density="compact"
          fixed-tabs
          align-tabs="center"
        >
          <v-tab value="sub">{{ $t('setting.sub') }}</v-tab>
          <v-tab value="link">{{ $t('client.links') }}</v-tab>
        </v-tabs>
	        <v-window v-model="tab" style="margin-top: 10px;">
	          <v-window-item value="sub">
            <v-row>
              <v-col style="text-align: center;">
                <v-chip>{{ $t('setting.sub') }}</v-chip><br />
                <QrcodeVue :value="clientSub" :size="size" @click="copyToClipboard(clientSub)" :margin="1" style="border-radius: 1rem; cursor: copy;" />
              </v-col>
            </v-row>
            <v-row>
              <v-col style="text-align: center;">
                <v-chip>{{ $t('setting.jsonSub') }}</v-chip><br />
                <QrcodeVue :value="clientSub + '?format=json'" :size="size" @click="copyToClipboard(clientSub + '?format=json')" :margin="1" style="border-radius: 1rem; cursor: copy;" />
              </v-col>
            </v-row>
            <v-row>
              <v-col style="text-align: center;">
                <v-chip>{{ $t('setting.clashSub') }}</v-chip><br />
                <QrcodeVue :value="clientSub + '?format=clash'" :size="size" @click="copyToClipboard(clientSub + '?format=clash')" :margin="1" style="border-radius: 1rem; cursor: copy;" />
              </v-col>
            </v-row>
	            <v-row>
	              <v-col style="text-align: center;">
	                <v-chip>SING-BOX (scan only)</v-chip><br />
	                <QrcodeVue :value="singbox" :size="size" :margin="1" style="border-radius: .8rem; cursor: not-allowed;" />
	              </v-col>
	            </v-row>
	            <template v-if="proxyHomeInbounds.length > 0">
	              <v-divider class="my-4"></v-divider>
	              <v-row>
	                <v-col>
	                  <v-select
	                    v-model="proxyHomeInboundId"
	                    :items="proxyHomeInbounds"
	                    item-title="tag"
	                    item-value="id"
	                    :label="$t('in.proxyHome') + ' ' + $t('objects.inbound')"
	                    hide-details
	                  />
	                </v-col>
	              </v-row>
	              <v-row v-if="selectedProxyHomeInbound">
	                <v-col style="text-align: center;">
	                  <v-chip>{{ $t('setting.jsonSub') }} · {{ selectedProxyHomeInbound.tag }}</v-chip><br />
	                  <QrcodeVue :value="proxyHomeJsonSub" :size="size" @click="copyToClipboard(proxyHomeJsonSub)" :margin="1" style="border-radius: 1rem; cursor: copy;" />
	                </v-col>
	              </v-row>
	              <v-row v-if="selectedProxyHomeInbound">
	                <v-col style="text-align: center;">
	                  <v-chip>{{ $t('setting.clashSub') }} · {{ selectedProxyHomeInbound.tag }}</v-chip><br />
	                  <QrcodeVue :value="proxyHomeClashSub" :size="size" @click="copyToClipboard(proxyHomeClashSub)" :margin="1" style="border-radius: 1rem; cursor: copy;" />
	                </v-col>
	              </v-row>
	              <v-row v-if="selectedProxyHomeInbound">
	                <v-col style="text-align: center;">
	                  <v-chip>SING-BOX ({{ selectedProxyHomeInbound.tag }})</v-chip><br />
	                  <QrcodeVue :value="proxyHomeSingbox" :size="size" :margin="1" style="border-radius: .8rem; cursor: not-allowed;" />
	                </v-col>
	              </v-row>
	            </template>
	          </v-window-item>
          <v-window-item value="link">
            <v-row v-for="l in clientLinks">
              <v-col style="text-align: center;">
                <v-chip>{{ l.remark?? $t('client.' + l.type) }}</v-chip><br />
                <QrcodeVue :value="l.uri" :size="size" @click="copyToClipboard(l.uri)" :margin="1" style="border-radius: .5rem; cursor: copy;" />
              </v-col>
            </v-row>
          </v-window-item>
        </v-window>
      </v-card-text>
    </v-card>
  </v-dialog>
</template>

<script lang="ts">
import QrcodeVue from 'qrcode.vue'
import Data from '@/store/modules/data'
import Clipboard from 'clipboard'
import { i18n } from '@/locales'
import { push } from 'notivue'

export default {
  props: ['id', 'visible'],
  data() {
	    return {
	      tab: "sub",
	      client: <any>{},
	      loading: false,
	      proxyHomeInboundId: null as number | null,
	    }
	  },
  methods: {
	    async load() {
	      this.loading = true
	      const newData = await Data().loadClients(this.$props.id)
	      this.client = newData
	      this.proxyHomeInboundId = this.proxyHomeInbounds[0]?.id ?? null
	      this.loading = false
	    },
    copyToClipboard(txt:string) {
      const hiddenButton = document.createElement('button')
      hiddenButton.className = 'clipboard-btn'
      document.body.appendChild(hiddenButton)

      const clipboard = new Clipboard('.clipboard-btn', {
        text: () => txt,
        container: document.getElementById('qrcode-modal')?? undefined
      });

      clipboard.on('success', () => {
        clipboard.destroy()
        push.success({
          message: i18n.global.t('success') + ": " + i18n.global.t('copyToClipboard'),
          duration: 5000,
        })
      })

      clipboard.on('error', () => {
        clipboard.destroy()
        push.error({
          message: i18n.global.t('failed') + ": " + i18n.global.t('copyToClipboard'),
          duration: 5000,
        })
      })

      // Perform click on hidden button to trigger copy
      hiddenButton.click()
      document.body.removeChild(hiddenButton)
    }
  },
  computed: {
    clientSub() {
      return Data().subURI + this.client.name
    },
	    singbox() {
	      const url = Data().subURI + this.client.name + "?format=json"
	      return "sing-box://import-remote-profile?url=" +  encodeURIComponent(url) + "#" + this.client.name
	    },
	    proxyHomeInbounds() {
	      const ids = this.client.inbounds ?? []
	      return (Data().inbounds ?? []).filter((inbound:any) => ids.includes(inbound.id) && inbound.proxy_home)
	    },
	    selectedProxyHomeInbound() {
	      return this.proxyHomeInbounds.find((inbound:any) => inbound.id === this.proxyHomeInboundId) ?? null
	    },
	    proxyHomeJsonSub() {
	      if (!this.selectedProxyHomeInbound) return ""
	      return this.clientSub + "?format=json&inbound=" + this.selectedProxyHomeInbound.id
	    },
	    proxyHomeClashSub() {
	      if (!this.selectedProxyHomeInbound) return ""
	      return this.clientSub + "?format=clash&inbound=" + this.selectedProxyHomeInbound.id
	    },
	    proxyHomeSingbox() {
	      if (!this.selectedProxyHomeInbound) return ""
	      return "sing-box://import-remote-profile?url=" + encodeURIComponent(this.proxyHomeJsonSub) + "#" + this.client.name + "-" + this.selectedProxyHomeInbound.tag
	    },
	    clientLinks() {
	      return this.client.links?? []
	    },
    size() {
      if (window.innerWidth > 380) return 300
      if (window.innerWidth > 330) return 280
      return 250
    }
  },
  watch: {
    visible(v) {
      if (v) {
        this.tab = "sub"
        this.load()
      }
    },
  },
  components: { QrcodeVue }
}
</script>
