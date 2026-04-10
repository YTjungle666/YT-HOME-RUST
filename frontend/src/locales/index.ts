import { createI18n } from 'vue-i18n'
import zhcn from './zhcn'

const supportedLocales = ['zhHans'] as const

const resolveLocale = (value: string | null | undefined) => {
  if (value && supportedLocales.includes(value as (typeof supportedLocales)[number])) {
    return value
  }
  return 'zhHans'
}

export const i18n = createI18n({
  legacy: false,
  locale: resolveLocale(localStorage.getItem('locale')),
  fallbackLocale: 'zhHans',
  messages: {
    zhHans: zhcn,
  },
})

export const locale = 'zh-cn'

export const languages = [
  { title: '简体中文', value: 'zhHans' },
]
