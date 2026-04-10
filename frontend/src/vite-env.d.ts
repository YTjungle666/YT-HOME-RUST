/// <reference types="vite/client" />

declare module 'moment/locale/zh-cn'

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}
