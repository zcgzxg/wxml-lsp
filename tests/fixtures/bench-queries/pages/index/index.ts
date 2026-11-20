interface User {
  /** 用户显示名称 */
  name: string;
  /** 用户年龄 */
  age: number;
  /** 用户邮箱 */
  email: string;
  /** 头像地址 */
  avatarUrl: string;
  /** 用户角色 */
  role: 'admin' | 'member';
}

interface Row {
  /** 行标题 */
  label: string;
  /** 行数值 */
  value: number;
}

Page({
  data: {
    /** 页面标题：query/type/root 锚点。显式标注以保证 hover 展示 string。 */
    title: '查询基准' as string,
    /** 当前用户：query/type/member 锚点使用 user.name。 */
    user: {
      name: 'Ada',
      age: 36,
      email: 'ada@example.com',
      avatarUrl: '/assets/avatar.png',
      role: 'admin',
    } as User,
    /** 混合 class 的 mustache 变量。 */
    extra: 'is-highlighted' as string,
    /** 列表数据：query/type/local 锚点使用 item.label。 */
    rows: [
      { label: '模板查询', value: 12 },
      { label: '类型查询', value: 18 },
      { label: '样式查询', value: 9 },
    ] as Row[],
  },
  onTap() {
    // bench 样本不包含真实交互。
  },
});
