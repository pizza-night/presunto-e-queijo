#!/bin/bash

if ! [[ $1 =~ peq1|peq2|peq3 ]]; then
    echo "Usage: $0 [peq1|peq2|peq3] <initial peer>"
    echo
    echo "[] are mandatory, <> are optional"
    exit 1
fi

if [[ "$2" ]]; then
    if [[ "$2" =~ peq1|peq2|peq3 ]]; then
        p_ip=$( sudo ip netns exec $2 ip -j -p a show dev veth-$2 | jq -r '.[0].addr_info[0].local')
        initial_peer=("-i" "$p_ip:2504")
    else
        initial_peer=("-i" "$2")
    fi

    echo "connecting to initial peer $p_ip"
fi

sudo ip netns exec $1 \
    ./target/debug/presunto-e-queijo \
    -u $1 "${initial_peer[@]}" \
    --debug-logs /tmp/$1
