import type { User } from './models';

Component({
  properties: {
    /** 标题 */
    title: { type: String, value: '' },
  },
  data: {
    /** 当前用户 */
    user: { name: 'Ada' } as User,
  },
});
