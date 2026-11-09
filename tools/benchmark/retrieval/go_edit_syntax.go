// Parse one SHA-pinned pre-edit package and its reconstructed anchor for
// edit2ripple's diagnostic candidates. This program has no repository writes.
package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io"
	"os"
	"sort"
	"strconv"
)

type request struct {
	Anchor     string            `json:"anchor"`
	Patched    string            `json:"patched"`
	AddedLines []int             `json:"added_lines"`
	Sources    map[string]string `json:"sources"`
}

type importRow struct {
	Path string `json:"path"`
}

type selectorRow struct {
	ReceiverType  string `json:"receiver_type"`
	RootName      string `json:"root_name"`
	ReceiverField string `json:"receiver_field"`
	TargetField   string `json:"target_field"`
}

type fieldRow struct {
	Name string `json:"name"`
	Type string `json:"type"`
}

type structRow struct {
	Path   string     `json:"path"`
	Name   string     `json:"name"`
	Fields []fieldRow `json:"fields"`
}

type response struct {
	Imports   []importRow   `json:"imports"`
	Selectors []selectorRow `json:"selectors"`
	Structs   []structRow   `json:"structs"`
}

func identifier(expr ast.Expr) string {
	if pointer, ok := expr.(*ast.StarExpr); ok {
		expr = pointer.X
	}
	if value, ok := expr.(*ast.Ident); ok {
		return value.Name
	}
	return ""
}

func inspect(input request) (response, error) {
	var result response
	result.Imports = []importRow{}
	result.Selectors = []selectorRow{}
	result.Structs = []structRow{}
	added := make(map[int]bool, len(input.AddedLines))
	for _, line := range input.AddedLines {
		if line <= 0 {
			return result, fmt.Errorf("invalid added line")
		}
		added[line] = true
	}
	fset := token.NewFileSet()
	original, ok := input.Sources[input.Anchor]
	if !ok {
		return result, fmt.Errorf("original anchor is absent")
	}
	old, err := parser.ParseFile(fset, input.Anchor, original, parser.ImportsOnly|parser.SkipObjectResolution)
	if err != nil {
		return result, fmt.Errorf("pinned anchor imports: %w", err)
	}
	oldImports := make(map[string]bool, len(old.Imports))
	for _, spec := range old.Imports {
		path, err := strconv.Unquote(spec.Path.Value)
		if err != nil {
			return result, fmt.Errorf("invalid pinned import path: %w", err)
		}
		oldImports[path] = true
	}
	// Object resolution binds each selector root to the actual receiver,
	// including in the presence of a shadowing local or closure parameter.
	anchor, err := parser.ParseFile(fset, input.Anchor, input.Patched, 0)
	if err != nil {
		return result, fmt.Errorf("patched anchor: %w", err)
	}
	for _, spec := range anchor.Imports {
		if added[fset.PositionFor(spec.Path.Pos(), false).Line] {
			path, err := strconv.Unquote(spec.Path.Value)
			if err != nil {
				return result, fmt.Errorf("invalid import path: %w", err)
			}
			if !oldImports[path] {
				result.Imports = append(result.Imports, importRow{Path: path})
			}
		}
	}
	for _, declaration := range anchor.Decls {
		method, ok := declaration.(*ast.FuncDecl)
		if !ok || method.Recv == nil || method.Body == nil || len(method.Recv.List) != 1 {
			continue
		}
		receiver := method.Recv.List[0]
		if len(receiver.Names) != 1 {
			continue
		}
		receiverType := identifier(receiver.Type)
		if receiverType == "" {
			continue
		}
		ast.Inspect(method.Body, func(node ast.Node) bool {
			outer, ok := node.(*ast.SelectorExpr)
			if !ok || !added[fset.PositionFor(outer.Sel.Pos(), false).Line] {
				return true
			}
			inner, ok := outer.X.(*ast.SelectorExpr)
			if !ok {
				return true
			}
			root, ok := inner.X.(*ast.Ident)
			if !ok || root.Obj == nil || root.Obj != receiver.Names[0].Obj {
				return true
			}
			result.Selectors = append(result.Selectors, selectorRow{
				ReceiverType:  receiverType,
				RootName:      root.Name,
				ReceiverField: inner.Sel.Name,
				TargetField:   outer.Sel.Name,
			})
			return true
		})
	}
	paths := make([]string, 0, len(input.Sources))
	for path := range input.Sources {
		paths = append(paths, path)
	}
	sort.Strings(paths)
	for _, path := range paths {
		file, err := parser.ParseFile(fset, path, input.Sources[path], parser.SkipObjectResolution)
		if err != nil {
			return result, fmt.Errorf("pinned package source %s: %w", path, err)
		}
		if file.Name.Name != anchor.Name.Name {
			return result, fmt.Errorf("package name differs in %s", path)
		}
		for _, declaration := range file.Decls {
			group, ok := declaration.(*ast.GenDecl)
			if !ok || group.Tok != token.TYPE {
				continue
			}
			for _, item := range group.Specs {
				typeSpec, ok := item.(*ast.TypeSpec)
				if !ok {
					continue
				}
				body, ok := typeSpec.Type.(*ast.StructType)
				if !ok {
					continue
				}
				row := structRow{Path: path, Name: typeSpec.Name.Name, Fields: []fieldRow{}}
				for _, field := range body.Fields.List {
					for _, name := range field.Names {
						row.Fields = append(row.Fields, fieldRow{Name: name.Name, Type: identifier(field.Type)})
					}
				}
				result.Structs = append(result.Structs, row)
			}
		}
	}
	return result, nil
}

func main() {
	decoder := json.NewDecoder(io.LimitReader(os.Stdin, 64<<20))
	decoder.DisallowUnknownFields()
	var input request
	if err := decoder.Decode(&input); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		fmt.Fprintln(os.Stderr, "trailing input")
		os.Exit(2)
	}
	output, err := inspect(input)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	if err := json.NewEncoder(os.Stdout).Encode(output); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}
