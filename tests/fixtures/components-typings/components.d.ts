// 组件类型声明样本。
//
// 位置约定：本文件必须与本工程的 tsconfig.json 同级；语言服务只在该位置
// 发现组件声明，放到子目录（例如 types/）不会生效。
//
// 入口是 WxmlComponentMap：只有被它引用的接口才会成为 WXML 标签的组件事实，
// 未被引用的接口（见文件末尾的 OrphanButtonProps）不产生任何效果。
// 类型只使用本项目声明子集内的写法：字面量联合、基础类型、嵌套对象。

declare global {
  interface WxmlComponentMap {
    'van-button': VanButtonProps;
  }

  interface VanButtonProps {
    props: {
      /** 按钮类型，字符串字面量联合 */
      type?: 'primary' | 'default' | 'warning';
      /** 是否显示加载中，可选 boolean */
      loading?: boolean;
      /** 按钮文案，必填字符串 */
      text: string;
    };
    events: {
      /** 点击确认后触发，detail 携带列表下标 */
      change: { detail: { index: number } };
      /** 输入时触发，用于事件名补全样本 */
      input: { detail: { value: string } };
    };
  }

  // 未生效样例：没有被 WxmlComponentMap 引用，不得产生组件事实。
  interface OrphanButtonProps {
    props: {
      unused: string;
    };
  }
}

export {};
