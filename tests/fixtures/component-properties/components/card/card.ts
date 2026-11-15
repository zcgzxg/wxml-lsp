Component({
  properties: {
    /** 标题，支持字符串或数字 */
    title: {
      type: String,
      optionalTypes: [Number],
      value: '',
    },
    /** 是否禁用 */
    disabled: {
      type: Boolean,
      value: false,
    },
  },
  data: {
    /** 内部状态 */
    state: 'idle',
  },
});
