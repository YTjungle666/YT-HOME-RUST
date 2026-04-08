package sub

import (
	"testing"

	"github.com/alireza0/s-ui/database/model"
)

func TestFilterSubscriptionInbounds(t *testing.T) {
	inbounds := []*model.Inbound{
		{Id: 1, Tag: "alpha"},
		{Id: 2, Tag: "beta"},
	}

	filtered, err := filterSubscriptionInbounds(inbounds, "")
	if err != nil {
		t.Fatalf("unexpected error for empty filter: %v", err)
	}
	if len(filtered) != 2 {
		t.Fatalf("expected all inbounds, got %d", len(filtered))
	}

	filtered, err = filterSubscriptionInbounds(inbounds, "2")
	if err != nil {
		t.Fatalf("unexpected error filtering by id: %v", err)
	}
	if len(filtered) != 1 || filtered[0].Tag != "beta" {
		t.Fatalf("expected beta inbound, got %#v", filtered)
	}

	filtered, err = filterSubscriptionInbounds(inbounds, "alpha")
	if err != nil {
		t.Fatalf("unexpected error filtering by tag: %v", err)
	}
	if len(filtered) != 1 || filtered[0].Id != 1 {
		t.Fatalf("expected alpha inbound, got %#v", filtered)
	}

	if _, err = filterSubscriptionInbounds(inbounds, "missing"); err == nil {
		t.Fatal("expected error for missing inbound")
	}
}
