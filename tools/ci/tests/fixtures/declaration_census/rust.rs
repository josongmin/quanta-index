// fn comment_decoy() {}
/// Doc example: `fn doc_decoy() {}`
const fn target() -> u8 { 1 }
async fn target_async() {}
pub(crate) unsafe extern "C" fn ffi_target() {}
struct Point { x: u8 }
enum Shape { Circle }
union Bits { a: u32 }
trait Draw { type Out; const K: u8; fn draw(&self); fn ready(&self) -> bool { true } }
impl Draw for Point { type Out = u8; const K: u8 = 1; fn draw(&self) {} }
type Alias = Point;
static COUNTER: u8 = 0;
const _: () = ();
mod inner { pub fn target() { fn nested() {} } }
macro_rules! make { () => {} }
extern "C" { fn ext_fn(); static EXT_VAL: u8; type Opaque; }
fn r#match() {}
fn café() {}
fn Target() {}
lazy_static! { static ref HIDDEN: u8 = 0; }
let_macro! { fn hidden_in_macro() {} }
const STR: &str = "fn string_decoy() {}";
