Component({
  properties: {
    /** 标题，支持字符串或数字 */
    title: { type: String, optionalTypes: [Number], value: '' },
  },
  data: {
    /** 内部状态 */
    state: 'idle',
  },
  methods: {
    onTap() {
      this.setData({ state: 'busy' });
    },
  },
});

export {};
