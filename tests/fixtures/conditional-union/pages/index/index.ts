interface User {
  /** 用户显示名称 */
  name: string;
}

type State =
  | { kind: 'loading' }
  | { kind: 'ready'; user: User }
  | {
      kind: 'error';
      /** 错误码 */
      code: number;
    };

Page({
  data: {
    /** 页面状态，判别联合 */
    state: { kind: 'loading' } as State,
    /** 可能为空的当前用户 */
    maybeUser: null as User | null,
  },
});
