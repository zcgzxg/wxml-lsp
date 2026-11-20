import type { User } from '@models/user';

Page({
  data: {
    /** 当前用户 */
    user: { name: 'Ada', age: 36 } as User,
  },
});
