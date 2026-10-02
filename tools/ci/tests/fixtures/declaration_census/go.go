package sample

// func Decoy() {}
func Target() {}
func (r *Recv) Method() {}
type Recv struct{}
type Alias = Recv
type Iface interface {
	Run() error
	fmt.Stringer
}
type (
	Grouped int
	Generic[T any] struct{ v T }
)
func _() {}
var NotDecl = 1
func outer() { type Local int }
type Anon struct{ f interface{ Hidden() } }
