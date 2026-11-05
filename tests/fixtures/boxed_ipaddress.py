from ipaddress import (
    collapse_addresses,
    ip_address,
    ip_interface,
    ip_network,
    summarize_address_range,
)


def current():
    network = ip_network("192.0.2.17/28", strict=False)
    print(network, network.with_netmask, network.num_addresses)
    print(ip_address("192.0.2.30") in network, ip_address("192.0.2.32") in network)
    left, right = network.subnets()
    print(left.subnet_of(network), network.overlaps(right), left.overlaps(right))
    print(*collapse_addresses([left, right]))
    print(*summarize_address_range(ip_address("192.0.2.4"), ip_address("192.0.2.9")))

    address = ip_address("2001:0db8:0:0:0:0:0:1")
    print(address, address.exploded, ip_address(address.packed) == address)
    print(ip_address("::ffff:192.0.2.1").ipv4_mapped)
    interface = ip_interface("2001:db8::9/64")
    print(interface.ip, interface.network, interface.with_prefixlen)

    for parse, value in [
        (ip_address, "192.0.2.256"),
        (ip_address, "2001:db8:::1"),
        (ip_network, "192.0.2.17/28"),
    ]:
        try:
            parse(value)
        except ValueError as error:
            print(type(error).__name__, error)


if __name__ == "__main__":
    current()
