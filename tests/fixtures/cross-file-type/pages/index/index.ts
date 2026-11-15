import type { User } from '../../types';

const user: User = { name: 'Ada', age: 36 };

Page({
  data: {
    /** 当前登录用户，类型来自 types.ts */
    user,
    /** 页面标题 */
    title: '用户资料',
  },
});
