const sharedMethods = {
  onTap() {},
};

Component({
  properties: {
    /** 标题 */
    title: { type: String, value: '' },
  },
  data: {
    /** 内部状态 */
    state: 'idle',
  },
  methods: sharedMethods,
});
