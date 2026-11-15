import type { User } from '@models/user';

const user: User = { name: 'Ada' };

Page({
  data: {
    /** 当前用户，通过 @models 别名导入类型 */
    user,
  },
});
