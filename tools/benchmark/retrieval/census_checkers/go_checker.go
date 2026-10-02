// Independent Go declaration census over go/ast (not Tree-sitter).
//
// Reads one file path per stdin line and writes one JSON object per line:
// {"path", "declarations": [[name, kind, byte_offset]]} or {"path", "error"}.
// It mirrors go_indexed_definition_v3: functions, methods, type specs and
// method elements of an interface that is directly a type spec's type.
package main

import (
	"bufio"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
)

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 1<<16), 1<<20)
	encoder := json.NewEncoder(os.Stdout)
	for scanner.Scan() {
		path := scanner.Text()
		out := map[string]any{"path": path}
		fset := token.NewFileSet()
		file, err := parser.ParseFile(fset, path, nil, parser.SkipObjectResolution)
		if err != nil {
			out["error"] = "parse: " + err.Error()
			_ = encoder.Encode(out)
			continue
		}
		rows := [][]any{}
		add := func(ident *ast.Ident, kind string) {
			rows = append(rows, []any{ident.Name, kind, fset.Position(ident.Pos()).Offset})
		}
		ast.Inspect(file, func(node ast.Node) bool {
			switch value := node.(type) {
			case *ast.FuncDecl:
				add(value.Name, "func")
			case *ast.TypeSpec:
				add(value.Name, "type")
				if iface, ok := value.Type.(*ast.InterfaceType); ok {
					for _, field := range iface.Methods.List {
						if _, method := field.Type.(*ast.FuncType); method {
							for _, name := range field.Names {
								add(name, "interface_method")
							}
						}
					}
				}
			}
			return true
		})
		out["declarations"] = rows
		_ = encoder.Encode(out)
	}
	if err := scanner.Err(); err != nil {
		os.Exit(2)
	}
}
