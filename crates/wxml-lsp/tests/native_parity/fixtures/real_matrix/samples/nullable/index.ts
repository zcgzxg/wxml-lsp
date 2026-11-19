interface User {
  /** 用户显示名称 */
  name: string;
}

Page({
  data: {
    maybeUser: null as User | null,
    /** 外部数据 */
    external: null as unknown,
    users: [] as User[],
  },
});
