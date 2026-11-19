// `module` comes from miniprogram-api-typings; this sample only adds a
// CommonJS-shaped statement so the page stays a script (no import/export).
module.exports = { appName: 'probe' };

Page({
  data: {
    /** 页面标题 */
    title: '首页',
    /** 当前用户 */
    user: {
      /** 用户显示名称 */
      name: 'Ada',
    },
  },
});
