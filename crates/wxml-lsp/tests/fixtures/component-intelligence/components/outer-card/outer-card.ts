Component({
  properties: {
    /** 计数，页面上传入 */
    count: { type: Number, value: 0 },
    /** 标题，支持字符串或数字 */
    title: { type: String, optionalTypes: [Number], value: '' },
    active: { type: Boolean, value: false },
  },
  data: {
    internal: 'state',
  },
});
