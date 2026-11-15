Page({
  data: {
    /** 页面标题 */
    title: '首页',
    /** 当前用户 */
    user: {
      /** 用户显示名称 */
      name: 'Ada',
      /** 用户年龄 */
      age: 36,
    },
  },
  onShow() {
    this.setData({ title: '欢迎回来' });
  },
});
