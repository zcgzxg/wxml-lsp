/**
 * @typedef {Object} User
 * @property {string} name 用户显示名称
 * @property {number} age 用户年龄
 */

/** @type {User} */
const user = { name: 'Ada', age: 36 };

/** @type {number} */
const unused = 'x';

Page({
  data: {
    /** 问候语 */
    greeting: '你好',
    user,
  },
});
