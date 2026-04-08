<template>
  <v-container fluid class="login-shell">
    <v-row class="fill-height" align="center" justify="center">
      <v-col cols="12" xl="10">
        <div class="login-grid">
          <section class="brand-panel">
            <div class="brand-copy">
              <span class="brand-kicker">YT HOME</span>
              <h1 class="brand-title">Build a clean return-home gateway for your private network.</h1>
              <p class="brand-body">
                Manage inbounds, clients, subscriptions and Reality-ready nodes from a single control plane built for real home access workflows.
              </p>
            </div>

            <div class="stats-grid">
              <div class="stat-card" v-for="stat in stats" :key="stat.label">
                <span class="stat-value">{{ stat.value }}</span>
                <span class="stat-label">{{ stat.label }}</span>
              </div>
            </div>

            <div class="feature-list">
              <article class="feature-card" v-for="feature in features" :key="feature.title">
                <div class="feature-icon">
                  <v-icon :icon="feature.icon" size="20" />
                </div>
                <div>
                  <h2>{{ feature.title }}</h2>
                  <p>{{ feature.body }}</p>
                </div>
              </article>
            </div>

            <div class="workflow-list">
              <span class="workflow-pill" v-for="item in workflowItems" :key="item">{{ item }}</span>
            </div>
          </section>

          <aside class="auth-panel">
            <v-card class="auth-card" rounded="xl" elevation="0">
              <div class="auth-header">
                <span class="auth-kicker">Control Plane</span>
                <h2>{{ $t('login.title') }}</h2>
                <p>Sign in to publish nodes, distribute subscriptions and keep your home network reachable.</p>
              </div>

              <v-form @submit.prevent="login" ref="form">
                <v-text-field
                  v-model="username"
                  :label="$t('login.username')"
                  :rules="usernameRules"
                  variant="outlined"
                  required
                />
                <v-text-field
                  v-model="password"
                  :label="$t('login.password')"
                  :rules="passwordRules"
                  type="password"
                  variant="outlined"
                  required
                />
                <v-btn
                  :loading="loading"
                  type="submit"
                  color="primary"
                  block
                  size="large"
                  class="mt-2"
                  v-text="$t('actions.submit')"
                />
              </v-form>

              <div class="auth-footer">
                <v-select
                  density="comfortable"
                  hide-details
                  variant="outlined"
                  :items="languages"
                  v-model="$i18n.locale"
                  @update:modelValue="changeLocale"
                />
                <v-menu>
                  <template v-slot:activator="{ props }">
                    <v-btn icon variant="outlined" v-bind="props">
                      <v-icon>mdi-theme-light-dark</v-icon>
                    </v-btn>
                  </template>
                  <v-list>
                    <v-list-item
                      v-for="th in themes"
                      :key="th.value"
                      @click="changeTheme(th.value)"
                      :prepend-icon="th.icon"
                      :active="isActiveTheme(th.value)"
                    >
                      <v-list-item-title>{{ $t(`theme.${th.value}`) }}</v-list-item-title>
                    </v-list-item>
                  </v-list>
                </v-menu>
              </div>
            </v-card>
          </aside>
        </div>
      </v-col>
    </v-row>
  </v-container>
</template>

<script lang="ts" setup>
import { ref } from "vue"
import { useLocale,useTheme } from 'vuetify'
import { i18n, languages } from '@/locales'
import { useRouter } from 'vue-router'
import HttpUtil from '@/plugins/httputil'


const theme = useTheme()
const locale = useLocale()

const themes = [
  { value: 'light', icon: 'mdi-white-balance-sunny' },
  { value: 'dark', icon: 'mdi-moon-waning-crescent' },
  { value: 'system', icon: 'mdi-laptop' },
]

const username = ref('')
const usernameRules = [
  (value: string) => {
    if (value?.length > 0) return true
    return i18n.global.t('login.unRules')
  },
]

const password = ref('')
const passwordRules = [
  (value: string) => {
    if (value?.length > 0) return true
    return i18n.global.t('login.pwRules')
  },
]

const loading = ref(false)
const router = useRouter()
const stats = [
  { value: 'Single Node', label: 'Proxy-home subscription mode' },
  { value: 'Reality Ready', label: 'Public access with clean TLS camouflage' },
  { value: 'Home First', label: 'Built for private network reachability' },
]
const features = [
  {
    icon: 'mdi-home-switch-outline',
    title: 'Proxy Home',
    body: 'Enable whole-home return access on a dedicated node without disturbing normal multi-node subscriptions.',
  },
  {
    icon: 'mdi-shield-lock-outline',
    title: 'Controlled Exposure',
    body: 'Keep panel control, subscription delivery and public entry points separated so publishing a node stays predictable.',
  },
  {
    icon: 'mdi-lan-connect',
    title: 'Operations View',
    body: 'Watch traffic, clients, system status and active endpoints from a single operational surface.',
  },
]
const workflowItems = [
  'Reality + VLESS',
  'Single-node home proxy',
  'Client subscription delivery',
  'Home service reachability',
]

const login = async () => {
  if (username.value == '' || password.value == '') return
  loading.value=true
  const response = await HttpUtil.post('api/login',{user: username.value, pass: password.value})
  if(response.success){
    setTimeout(() => {
      loading.value=false
      router.push('/')
    }, 500)
  } else {
    loading.value=false
  }
}
const changeLocale = (l: any) => {
  locale.current.value = l ?? 'en'
  localStorage.setItem('locale', locale.current.value)
}
const changeTheme = (th: string) => {
  theme.change(th)
  localStorage.setItem('theme', th)
}
const isActiveTheme = (th: string) => {
  const current = localStorage.getItem('theme') ?? 'system'
  return current == th
}
</script>

<style scoped>
.login-shell {
  min-height: 100vh;
  padding: 24px;
  background:
    radial-gradient(circle at top left, rgba(25, 146, 169, 0.18), transparent 30%),
    radial-gradient(circle at bottom right, rgba(244, 162, 97, 0.18), transparent 28%),
    linear-gradient(135deg, #0f1720 0%, #13283a 52%, #1f3c43 100%);
}

.login-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.35fr) minmax(340px, 440px);
  gap: 24px;
  align-items: stretch;
}

.brand-panel {
  padding: 40px;
  border-radius: 28px;
  color: #f7fafc;
  background: linear-gradient(160deg, rgba(8, 15, 28, 0.76), rgba(18, 46, 61, 0.58));
  border: 1px solid rgba(255, 255, 255, 0.08);
  box-shadow: 0 28px 60px rgba(0, 0, 0, 0.25);
  backdrop-filter: blur(18px);
}

.brand-kicker,
.auth-kicker {
  display: inline-flex;
  align-items: center;
  padding: 6px 12px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.16em;
  text-transform: uppercase;
}

.brand-kicker {
  color: #08131f;
  background: linear-gradient(90deg, #ffd166, #f4a261);
}

.brand-title {
  margin: 18px 0 14px;
  max-width: 11ch;
  font-size: clamp(2.5rem, 4vw, 4.4rem);
  line-height: 0.95;
  letter-spacing: -0.04em;
}

.brand-body {
  max-width: 60ch;
  margin: 0;
  color: rgba(247, 250, 252, 0.82);
  font-size: 1rem;
  line-height: 1.7;
}

.stats-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 14px;
  margin-top: 28px;
}

.stat-card,
.feature-card {
  border: 1px solid rgba(255, 255, 255, 0.08);
  background: rgba(255, 255, 255, 0.05);
  border-radius: 22px;
}

.stat-card {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 18px;
}

.stat-value {
  font-size: 1.1rem;
  font-weight: 700;
}

.stat-label {
  color: rgba(247, 250, 252, 0.68);
  font-size: 0.92rem;
  line-height: 1.5;
}

.feature-list {
  display: grid;
  gap: 14px;
  margin-top: 22px;
}

.feature-card {
  display: grid;
  grid-template-columns: 44px minmax(0, 1fr);
  gap: 14px;
  padding: 18px;
}

.feature-card h2 {
  margin: 0 0 6px;
  font-size: 1rem;
}

.feature-card p {
  margin: 0;
  color: rgba(247, 250, 252, 0.7);
  line-height: 1.6;
}

.feature-icon {
  display: grid;
  place-items: center;
  width: 44px;
  height: 44px;
  border-radius: 14px;
  color: #08131f;
  background: linear-gradient(135deg, #7bdff2, #ffd166);
}

.workflow-list {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin-top: 24px;
}

.workflow-pill {
  padding: 10px 14px;
  border-radius: 999px;
  color: #f7fafc;
  font-size: 0.9rem;
  background: rgba(255, 255, 255, 0.08);
}

.auth-panel {
  display: flex;
}

.auth-card {
  width: 100%;
  padding: 30px;
  align-self: center;
  background: rgba(255, 252, 245, 0.96);
  box-shadow: 0 28px 60px rgba(0, 0, 0, 0.2);
}

.auth-header h2 {
  margin: 14px 0 10px;
  font-size: 2rem;
  line-height: 1;
  letter-spacing: -0.03em;
}

.auth-header p {
  margin: 0 0 24px;
  color: rgba(32, 43, 58, 0.72);
  line-height: 1.6;
}

.auth-kicker {
  color: #165a72;
  background: rgba(123, 223, 242, 0.18);
}

.auth-footer {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  margin-top: 18px;
}

@media (max-width: 1100px) {
  .login-grid {
    grid-template-columns: 1fr;
  }

  .brand-title {
    max-width: none;
  }
}

@media (max-width: 760px) {
  .login-shell {
    padding: 12px;
  }

  .brand-panel,
  .auth-card {
    padding: 22px;
  }

  .stats-grid {
    grid-template-columns: 1fr;
  }

  .auth-footer {
    grid-template-columns: 1fr;
  }
}
</style>
