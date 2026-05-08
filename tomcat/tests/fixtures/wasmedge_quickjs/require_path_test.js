// TASK-05a a.2：验证 wasmedge-quickjs modules/ 预挂载后 require('path') 可用
const path = require('path');
const j = path.join('a', 'b');
if (j !== 'a/b' && j !== 'a\\b') {
  throw new Error('path.join unexpected: ' + j);
}
print('path_ok');
