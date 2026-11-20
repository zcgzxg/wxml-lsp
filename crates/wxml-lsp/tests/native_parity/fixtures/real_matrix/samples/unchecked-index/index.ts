interface User {
  /** 用户显示名称 */
  name: string;
}

Page({
  data: {
    users: [] as User[],
  },
});
