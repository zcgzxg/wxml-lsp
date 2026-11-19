const base = {
  /** 基础尺寸 */
  size: { type: Number, value: 0 },
};

Component({
  properties: {
    ...base,
    /** 标题，支持字符串或数字 */
    title: { type: String, optionalTypes: [Number], value: '' },
  },
  data: {
    /** 内部状态 */
    state: 'idle',
  },
});
