// function decoy() {}
function target() {}
function* gen() {}
async function target2() {}
class Klass extends Base {
  constructor() { super(); }
  static make() {}
  #hidden() {}
  get prop() { return 1; }
}
const obj = { method() {}, ['computed']() {} };
const expr = class Named { inner() {} };
export default function () {}
const f = function named() {};
module.exports = { exported() {} };
function _() {}
