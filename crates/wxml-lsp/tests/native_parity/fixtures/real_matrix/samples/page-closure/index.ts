function registerPage(flag: boolean): void {
  interface User {
    /** 用户显示名称 */
    name: string;
    /** 用户年龄 */
    age: number;
  }

  type PageConfig = {
    /** 页面标题 */
    title: string;
  };

  const config: PageConfig = { title: '首页' };

  function identity<T>(value: T): T {
    return value;
  }

  if (!flag) return;

  Page({
    data: identity({
      config,
      user: { name: 'Ada', age: 36 } as User,
      items: [] as User[],
    }),
  });
}

registerPage(true);
