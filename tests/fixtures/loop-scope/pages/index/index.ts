interface Member {
  /** 成员名称 */
  name: string;
}

interface Group {
  /** 组名 */
  name: string;
  /** 组标题 */
  title: string;
  /** 成员数量 */
  memberCount: number;
  /** 成员列表 */
  members: Member[];
}

Page({
  data: {
    /** 分组列表 */
    groups: [] as Group[],
    /** 以组名为键的分组表 */
    groupMap: {} as Record<string, Group>,
  },
});
