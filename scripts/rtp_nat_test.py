# /// script
# requires-python = ">=3.14"
# dependencies = []
# ///
"""
Test CGNAT behavior for RTP port by sending SIP OPTIONS from the RTP socket.
This tells us what external port T-Mobile CGNAT assigns when our RTP socket
talks to the voip.ms IP address.
"""
import socket
import sys
import random
import time

VOIP_MS_IP = "208.100.60.38"
SIP_PORT = 5080

def main():
    # Create UDP socket (simulating RTP socket)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("0.0.0.0", 0))
    local_port = sock.getsockname()[1]
    print(f"Local RTP socket bound to port {local_port}")

    # Send SIP OPTIONS from this socket to voip.ms SIP port
    # The Via rport in the response tells us our CGNAT-mapped port
    branch = f"z9hG4bK{random.randint(0, 2**64):016x}"
    call_id = f"{random.randint(0, 2**64):016x}@rtp-test"
    tag = f"{random.randint(0, 2**32):08x}"

    options = (
        f"OPTIONS sip:ping@{VOIP_MS_IP}:{SIP_PORT} SIP/2.0\r\n"
        f"Via: SIP/2.0/UDP 0.0.0.0:{local_port};branch={branch};rport\r\n"
        f"From: <sip:test@rtp-test>;tag={tag}\r\n"
        f"To: <sip:ping@{VOIP_MS_IP}>\r\n"
        f"Call-ID: {call_id}\r\n"
        f"CSeq: 1 OPTIONS\r\n"
        f"Max-Forwards: 70\r\n"
        f"Content-Length: 0\r\n"
        f"\r\n"
    )

    print(f"\nSending SIP OPTIONS from :{local_port} to {VOIP_MS_IP}:{SIP_PORT}")
    sock.sendto(options.encode(), (VOIP_MS_IP, SIP_PORT))

    sock.settimeout(5.0)
    try:
        data, addr = sock.recvfrom(4096)
        response = data.decode("utf-8", errors="replace")
        print(f"\nReceived response from {addr}:")
        # Print just the Via line and status
        lines = response.split("\r\n")
        print(f"  Status: {lines[0]}")
        for line in lines:
            if line.lower().startswith("via:"):
                print(f"  {line}")
                # Extract received and rport
                if "received=" in line.lower():
                    parts = line.split(";")
                    for p in parts:
                        if "received=" in p.lower():
                            print(f"  -> Our external IP: {p.split('=')[1]}")
                        if "rport=" in p.lower():
                            rport = p.split("=")[1].strip()
                            print(f"  -> Our external port: {rport}")
                            print(f"\n  SDP should use: {p.split('=')[1] if 'received=' in p else '???'}:{rport}")
    except socket.timeout:
        print("  No response (timeout after 5s)")
        print("  -> T-Mobile may be blocking outbound UDP to voip.ms")

    # Now also test UDP to a typical RTP port range
    print(f"\n--- Testing UDP to high port (simulating RTP media port) ---")
    test_port = 15000  # typical Asterisk RTP port range
    sock2 = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock2.bind(("0.0.0.0", 0))
    local_port2 = sock2.getsockname()[1]

    # Send a few UDP packets to a high port
    dummy = b"\x80\x00\x00\x01\x00\x00\x00\xa0\x00\x00\x00\x01"  # RTP header
    print(f"Sending RTP-like packet from :{local_port2} to {VOIP_MS_IP}:{test_port}")
    sock2.sendto(dummy, (VOIP_MS_IP, test_port))

    sock2.settimeout(3.0)
    try:
        data, addr = sock2.recvfrom(4096)
        print(f"  Received response: {len(data)} bytes from {addr}")
    except socket.timeout:
        print(f"  No response (expected - port {test_port} probably not listening)")

    # Test sending to the actual SIP port from the second socket to check CGNAT mapping
    print(f"\n--- Checking CGNAT mapping consistency ---")
    options2 = options.replace(f":{local_port}", f":{local_port2}")
    branch2 = f"z9hG4bK{random.randint(0, 2**64):016x}"
    options2 = options.replace(branch, branch2).replace(f":{local_port}", f":{local_port2}")
    sock2.sendto(options2.encode(), (VOIP_MS_IP, SIP_PORT))

    sock2.settimeout(5.0)
    try:
        data, addr = sock2.recvfrom(4096)
        response = data.decode("utf-8", errors="replace")
        lines = response.split("\r\n")
        print(f"  Status: {lines[0]}")
        for line in lines:
            if line.lower().startswith("via:"):
                print(f"  {line}")
                for p in line.split(";"):
                    if "received=" in p.lower():
                        print(f"  -> Socket 2 external IP: {p.split('=')[1]}")
                    if "rport=" in p.lower():
                        print(f"  -> Socket 2 external port: {p.split('=')[1].strip()}")
    except socket.timeout:
        print("  No response (timeout)")

    sock.close()
    sock2.close()

if __name__ == "__main__":
    main()
