#!/bin/bash
# Fake JLinkExe for tests: answers ShowEmuList with a canned two-probe bench.
# Drains stdin (the Commander script) and ignores argv.
cat >/dev/null
echo 'SEGGER J-Link Commander V9.24a'
echo 'J-Link Command File read successfully.'
echo 'Processing script file...'
echo 'J-Link>ShowEmuList'
echo 'J-Link[0]: Connection: USB, Serial number: 1160002965, ProductName: J-Link-OB-Test, Nickname: apollo510b'
echo 'J-Link[1]: Connection: USB, Serial number: 1160003881, ProductName: J-Link-OB-Apollo4-CortexM, Nickname: <not set>'
echo 'J-Link>Exit'
echo 'Script processing completed.'
exit 0
