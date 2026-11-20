Page({
  data: {
    /** 按钮文案 */
    buttonText: '提交',
    /** 按钮是否加载中 */
    loading: true,
  },

  /** 声明中 change 的 detail 为 { index: number }，此标注与之兼容 */
  onButtonChange(event: WechatMiniprogram.CustomEvent<{ index: number }>) {
    console.log(event.detail.index);
  },
});
