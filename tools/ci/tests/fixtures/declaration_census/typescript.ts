// function commentDecoy() {}
/** @example function docDecoy() {} */
export function target(): void {}
function overloaded(a: string): void;
function overloaded(a: any) {}
declare function ambient(a: number): void;
export default function () {}
function* generate() {}
async function target2() {}
export abstract class Base {
  abstract run(): void;
  constructor() {}
  get value() { return 1; }
  set value(v: number) {}
  #secret() {}
  'quoted'() {}
  [Symbol.iterator]() {}
  helper(): void;
  helper() {}
  static create() {}
  handler = () => 1;
}
interface Shape { area(): number; name: string }
type Fn = { call(): void };
enum Color { Red }
namespace NS { export function nsFn() {} }
const obj = { method() {}, async *agen() {}, arrow: () => 1 };
const expr = class Named { inside() {} };
const fnExpr = function named() {};
const arrow = () => {};
declare module "ext" { function modFn(): void; }
class Café {}
const decoy = "function stringDecoy() {}";
