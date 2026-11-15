declare const sdkPayload: any;
declare const externalPayload: unknown;

Page({
  data: {
    /** 有明确类型的字段 */
    label: '示例',
    /** 来自未类型化 SDK 的任意值 */
    payload: sdkPayload,
    /** 尚未确认形状的外部数据 */
    external: externalPayload,
  },
});
