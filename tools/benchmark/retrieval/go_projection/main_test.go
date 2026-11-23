package main

import (
	"bytes"
	"testing"
)

func TestProjectionPreservesOriginalByteOffsets(t *testing.T) {
	source := []byte("package p\n// café\nfunc Locate(){_=new(1);_=make([]int,2);_=make(1)}\n")
	result := project(source)
	if result.Status != "ok" || len(result.Edits) != 2 {
		t.Fatalf("unexpected projection: %+v", result)
	}
	for index, expected := range []struct{ original, replacement string }{
		{"new", "foo"}, {"make", "fooo"},
	} {
		candidate := result.Edits[index]
		if string(source[candidate.Start:candidate.End]) != expected.original ||
			candidate.Replacement != expected.replacement {
			t.Fatalf("edit %d does not identify its original call: %+v", index, candidate)
		}
	}
	if result.Edits[0].Start != bytes.Index(source, []byte("new(1)")) ||
		result.Edits[1].Start != bytes.LastIndex(source, []byte("make(1)")) {
		t.Fatalf("byte offsets do not account for non-ASCII source: %+v", result.Edits)
	}
}

func TestInvalidOriginalCannotProduceEdits(t *testing.T) {
	result := project([]byte("package p\nfunc Broken(){_=new(,2)}\n"))
	if result.Status != "parse_failure" || len(result.Edits) != 0 {
		t.Fatalf("invalid original source was projected: %+v", result)
	}
}

func TestOversizeOriginalCannotProduceEdits(t *testing.T) {
	result := project(bytes.Repeat([]byte{'x'}, maxSourceBytes+1))
	if result.Status != "resource_limit" || len(result.Edits) != 0 {
		t.Fatalf("oversize original source was projected: %+v", result)
	}
}
