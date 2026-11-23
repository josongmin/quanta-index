// Command go_projection validates original Go syntax and reports only
// same-width call-callee substitutions needed by the released grammar.
// It never emits source bytes or changes a declaration name.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"io"
	"os"
	"runtime"
	"sort"
)

const maxSourceBytes = 1 << 20
const maxEdits = 1 << 16

type edit struct {
	Start       int    `json:"start"`
	End         int    `json:"end"`
	Replacement string `json:"replacement"`
}

type response struct {
	Schema       string `json:"schema"`
	Status       string `json:"status"`
	GoVersion    string `json:"go_version"`
	SourceSHA256 string `json:"source_sha256"`
	Edits        []edit `json:"edits"`
}

func replacementFor(call *ast.CallExpr) string {
	name, ok := call.Fun.(*ast.Ident)
	if !ok {
		return ""
	}
	// The released grammar already recognizes built-in type arguments.
	// Replacing `make([]int, 2)` with an ordinary call would introduce an
	// ERROR where the original source had none.
	if len(call.Args) > 0 {
		switch call.Args[0].(type) {
		case *ast.ArrayType, *ast.MapType, *ast.ChanType, *ast.FuncType,
			*ast.StructType, *ast.InterfaceType:
			return ""
		}
	}
	switch name.Name {
	case "new":
		return "foo"
	case "make":
		return "fooo"
	default:
		return ""
	}
}

func project(source []byte) response {
	digest := sha256.Sum256(source)
	result := response{
		Schema:       "quanta-go-grammar-projection-v1",
		Status:       "ok",
		GoVersion:    runtime.Version(),
		SourceSHA256: hex.EncodeToString(digest[:]),
		Edits:        []edit{},
	}
	if len(source) > maxSourceBytes {
		result.Status = "resource_limit"
		return result
	}
	files := token.NewFileSet()
	file, err := parser.ParseFile(files, "source.go", source, parser.AllErrors)
	if err != nil {
		result.Status = "parse_failure"
		return result
	}
	ast.Inspect(file, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		replacement := replacementFor(call)
		if replacement == "" {
			return true
		}
		callee := call.Fun.(*ast.Ident)
		start := files.PositionFor(callee.Pos(), false).Offset
		end := files.PositionFor(callee.End(), false).Offset
		result.Edits = append(result.Edits, edit{start, end, replacement})
		return true
	})
	if len(result.Edits) > maxEdits {
		result.Edits = []edit{}
		result.Status = "resource_limit"
		return result
	}
	sort.Slice(result.Edits, func(left, right int) bool {
		return result.Edits[left].Start < result.Edits[right].Start
	})
	for index, candidate := range result.Edits {
		if candidate.Start < 0 || candidate.End > len(source) ||
			candidate.End-candidate.Start != len(candidate.Replacement) ||
			(index > 0 && result.Edits[index-1].End > candidate.Start) {
			result.Edits = []edit{}
			result.Status = "producer_defect"
			break
		}
	}
	return result
}

func main() {
	if len(os.Args) != 1 {
		os.Exit(2)
	}
	source, err := io.ReadAll(io.LimitReader(os.Stdin, maxSourceBytes+1))
	if err != nil {
		os.Exit(2)
	}
	if err := json.NewEncoder(os.Stdout).Encode(project(source)); err != nil {
		os.Exit(2)
	}
}
