/** IP literal parsing and the address ranges a resolved public name must never reach. */
import { isIP } from "node:net";

/** Four octets for IPv4, or the IPv4 embedded in an IPv4-mapped/NAT64 IPv6 address. */
function ipv4Octets(address: string): number[] | undefined {
	if (isIP(address) === 4) return address.split(".").map(Number);
	const hextets = ipv6Hextets(address);
	if (!hextets) return undefined;
	const prefix = hextets.slice(0, 6);
	const mapped = prefix.slice(0, 5).every(value => value === 0) && prefix[5] === 0xffff;
	const nat64 = hextets[0] === 0x64 && hextets[1] === 0xff9b && hextets.slice(2, 6).every(value => value === 0);
	if (!mapped && !nat64) return undefined;
	const high = hextets[6] ?? 0;
	const low = hextets[7] ?? 0;
	return [high >> 8, high & 0xff, low >> 8, low & 0xff];
}

/** Eight hextets of an IPv6 literal (brackets and zone ids allowed), or undefined. */
export function ipv6Hextets(address: string): number[] | undefined {
	const bare = address.replace(/^\[|\]$/g, "").replace(/%.*$/, "");
	if (isIP(bare) !== 6) return undefined;
	let text = bare.toLowerCase();
	const dotted = /(\d+\.\d+\.\d+\.\d+)$/.exec(text);
	if (dotted?.[1]) {
		const [a, b, c, d] = dotted[1].split(".").map(Number);
		text = `${text.slice(0, -dotted[1].length)}${(((a ?? 0) << 8) | (b ?? 0)).toString(16)}:${(((c ?? 0) << 8) | (d ?? 0)).toString(16)}`;
	}
	const [head = "", tail] = text.split("::");
	const left = head ? head.split(":") : [];
	const right = tail ? tail.split(":") : [];
	const missing = 8 - left.length - right.length;
	if (tail === undefined ? left.length !== 8 : missing < 0) return undefined;
	return [...left, ...Array<string>(tail === undefined ? 0 : missing).fill("0"), ...right].map(part =>
		Number.parseInt(part, 16),
	);
}

/** Canonical text of an IP literal for comparison with allow-list entries. */
export function canonicalIp(address: string): string | undefined {
	const bare = address.replace(/^\[|\]$/g, "");
	if (isIP(bare) === 4) return bare;
	const hextets = ipv6Hextets(bare);
	return hextets && new URL(`http://[${hextets.map(value => value.toString(16)).join(":")}]/`).hostname.slice(1, -1);
}

export function isLoopbackAddress(address: string): boolean {
	const octets = ipv4Octets(address);
	if (octets) return octets[0] === 127;
	const hextets = ipv6Hextets(address);
	return hextets?.slice(0, 7).every(value => value === 0) === true && hextets[7] === 1;
}

/**
 * Loopback, private, link-local (cloud metadata), CGNAT, unspecified,
 * benchmarking, multicast and reserved ranges: never a public provider.
 */
export function isInternalAddress(address: string): boolean {
	const octets = ipv4Octets(address);
	if (octets) {
		const [a = 0, b = 0, c = 0] = octets;
		return (
			a === 0 ||
			a === 10 ||
			a === 127 ||
			(a === 100 && b >= 64 && b <= 127) ||
			(a === 169 && b === 254) ||
			(a === 172 && b >= 16 && b <= 31) ||
			(a === 192 && b === 0 && c === 0) ||
			(a === 192 && b === 168) ||
			(a === 198 && (b === 18 || b === 19)) ||
			a >= 224
		);
	}
	const hextets = ipv6Hextets(address);
	if (!hextets) return true;
	const first = hextets[0] ?? 0;
	return (
		hextets.every(value => value === 0) ||
		isLoopbackAddress(address) ||
		(first & 0xfe00) === 0xfc00 ||
		(first & 0xffc0) === 0xfe80 ||
		(first & 0xff00) === 0xff00
	);
}
