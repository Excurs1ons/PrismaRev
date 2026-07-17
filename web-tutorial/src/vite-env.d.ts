/// <reference types="vite/client" />

// 让 TS 认识 `?raw` 导入的 markdown 文件
declare module "*.md?raw" {
  const content: string;
  export default content;
}
