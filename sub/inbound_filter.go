package sub

import (
	"fmt"
	"strconv"

	"github.com/alireza0/s-ui/database/model"
)

func filterSubscriptionInbounds(inbounds []*model.Inbound, inboundRef string) ([]*model.Inbound, error) {
	if inboundRef == "" {
		return inbounds, nil
	}

	if inboundID, err := strconv.ParseUint(inboundRef, 10, 64); err == nil {
		for _, inbound := range inbounds {
			if inbound != nil && inbound.Id == uint(inboundID) {
				return []*model.Inbound{inbound}, nil
			}
		}
	}

	for _, inbound := range inbounds {
		if inbound != nil && inbound.Tag == inboundRef {
			return []*model.Inbound{inbound}, nil
		}
	}

	return nil, fmt.Errorf("inbound %q not found in subscription", inboundRef)
}
