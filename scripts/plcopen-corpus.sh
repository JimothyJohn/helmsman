#!/usr/bin/env bash
# Fetches real PLCopen TC6 projects saved by Beremiz and the OpenPLC Editor
# (their own example and test projects), checks each file's SHA-256, and
# runs crates/helmsman-iec/tests/plcopen_corpus.rs against them. The files
# are GPL (Beremiz: GPL-2.0/GPL-3.0, OpenPLC Editor: GPL-2.0), so they are
# fetched into target/ rather than committed; provenance is in
# docs/SOURCES.md. They are data only: nothing from them is executed.
#
# Needs: curl, shasum (or sha256sum), cargo, network on the first run.
# Idempotent: files already present with the right hash are not fetched.
# Corpus location: $HELMSMAN_PLCOPEN_CORPUS (default target/plcopen-corpus).
set -euo pipefail
cd "$(dirname "$0")/.."

corpus="${HELMSMAN_PLCOPEN_CORPUS:-$PWD/target/plcopen-corpus}"

# Corpus name -> raw-file base URL at a pinned commit.
url_of() {
  case "$1" in
    beremiz) echo "https://raw.githubusercontent.com/beremiz/beremiz/d7ddfb7523a53b153d74950c06c0447118f434c0" ;;
    # The OpenPLC Editor's IDE source: submodule `editor` of
    # thiagoralves/OpenPLC_Editor at 2b4add40ec9bdb84d0768503ce26a6b7517268ba.
    openplc) echo "https://raw.githubusercontent.com/thiagoralves/editor/6a8e930a914e4421e8fa05966b31eb371b439243" ;;
    *)
      echo "unknown corpus $1" >&2
      exit 1
      ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

# corpus  sha256  path
manifest="
beremiz 93ffd23cc8e51d7da9b25e3338764a05553886cfc96041e0ac628f6ac4e7fad0 exemples/canopen_master_slave/plc.xml
beremiz cf869b0c722461ef32264369ac4e48d46636ee48d10e5ae1334afe05999d66b7 exemples/csv_by_string/plc.xml
beremiz 6ca1278088ac0ba77cb259efaaf586490b18ef6b2fc677603fce45f3dbb1d550 exemples/csv_read/plc.xml
beremiz cb403f551963ea1064acc1aed8daa626eb2d51e0aae9f3784ac7895bb0cd5a01 exemples/first_steps/plc.xml
beremiz 71edc38dc7c32c62d2163dcc171d406ad3079cb9b04fafc216d5a0868c507adc exemples/modbus/plc.xml
beremiz b212fffb7f1ecfb611f1b8f26ec5d0b7eefd179b76c4c429873bb3af6a6d99ad exemples/python/plc.xml
beremiz 03eb8e002f36913d25530010296c78591b857ecb2770339e0f1e78cb32da62d4 exemples/svghmi_csv_json_img_table/plc.xml
beremiz b232fcac132e211992a55ebdd7f2246a6621483d4016f42213b1a2fc9cd4b58c exemples/svghmi_dropdown/plc.xml
beremiz ea54a478912a9665114d4f3164f91517739aea9aad3e1465dac3ce540a9707a9 exemples/svghmi_foreach/plc.xml
beremiz 2a9662cd860e8cc9369f0195405b69edc69e8288507c326f60fc61355e988c1e exemples/svghmi_json_table/plc.xml
beremiz f20845be5efc353f6931406e32fdc3ed1f9e0a38d915830e7e6c10471b12b310 exemples/svghmi_jumps/plc.xml
beremiz cc1ada616e4b23b6d14bccbd9e156bc6c1eb0ec161ff426fb1b521054f186443 exemples/svghmi_references/plc.xml
beremiz f7e22a67fa51cabd0884de1232f2c3b4f02de7902b0e32e6f1584e710d86c375 exemples/svghmi_traffic_light/plc.xml
beremiz c34bfde3652082988d3ef234fdf1028e356bed638504d50e4a576f61520b3715 tests/projects/BACnet/plc.xml
beremiz 49ebfb5088201d2ff800ad1c2819413470ce4adb05d63e72abf7620f531c3615 tests/projects/c_runtime/plc.xml
beremiz 2c8b42363393ac1f84bc1761e293e80b88ab6fd3b144f8f539ba7c0f47cbe3a7 tests/projects/canopen_master/plc.xml
beremiz 9809e4da432ddeddb221ee058dd794fef1ff9dcc523b7db764b70d7098b28e88 tests/projects/canopen_slave/plc.xml
beremiz cb3507067089775713b77786af6cc8dd18e2321794ac9a2bcfdc78216db11f00 tests/projects/canopen_systec/plc.xml
beremiz 5def0e18554ea48384e5510298827338d60fc4f028c84d9de9aaa220b5f8aa6c tests/projects/canopen_systec_faulhaber/plc.xml
beremiz 9a6ac7df13aa616d41b637c588f14f08c079b13ea61670aa51c8bb3ae80b736f tests/projects/genericmake/plc.xml
beremiz 5231dfaaeaf18ed1dd349b7b35345d0362b286dd7501c5ca75c31866dd3b5bb1 tests/projects/iec61131_lang_test/plc.xml
beremiz 00dbfa8a087e83cb3188398e490bbb246457b221e054a9b087dd5875439a3daf tests/projects/logging/plc.xml
beremiz 302daeb2392cb0fcb616d26fa52ae54c3bf25b498cc887794c8e731fed31843b tests/projects/modbus_test_tcp/plc.xml
beremiz 848fad61e6bb3b551af41351a8d53558136182d21f9a6c4714cf98684120d6b8 tests/projects/mqtt_client/plc.xml
beremiz 411e71e98d513e06a5c9936a01e2e5005f96a0656fc90522f07dfc57e598f57e tests/projects/mqtt_ssl/plc.xml
beremiz 32ca94bfe3e9da84e939d6d56a4f5db0734ebb35fbf58fd0ed6f59f718177630 tests/projects/opcua_browse/plc.xml
beremiz 32ca94bfe3e9da84e939d6d56a4f5db0734ebb35fbf58fd0ed6f59f718177630 tests/projects/opcua_browse_encrypted/plc.xml
beremiz 3c5da3ca487b274d6a6261de6654c23d4fa7ff3a37b5ab7a5bfb122454328c93 tests/projects/opcua_client/plc.xml
beremiz 050084371adaac99faf95c10e6d529288b4113a180e54e5bac2f9011f4f7e2ce tests/projects/opcua_client_encrypted/plc.xml
beremiz 050084371adaac99faf95c10e6d529288b4113a180e54e5bac2f9011f4f7e2ce tests/projects/opcua_client_erpc/plc.xml
beremiz f4c52fb18301af826014a18832e15c2127f479e71b444652b24b96c1691c70f4 tests/projects/plc_sdk_minimal/plc.xml
beremiz a2ad7a60b47effc0767e8837f4dac7646e260681f66a86bbb7bd9f7066226e01 tests/projects/python_legacy_plcbinary/plc.xml
beremiz 3ee7bdbc8bbaf0b6c46d008ad31150342f434949ec2cee8b2264c41b1d44de1a tests/projects/svghmi/plc.xml
beremiz ab088fe38b7c1baa06a20d6942f69ba817417b81092358cda15dfc82bc517ec4 tests/projects/svghmi_basic/plc.xml
beremiz dae8c0054ee7902045715791bda13d578d879ec10bd899a0d0c1bf42f98176a2 tests/projects/svghmi_i18n/plc.xml
beremiz a353ed6e672e43e26ae1c1c68f023f4d13bba559c855ad3436a3c8870f1e3d68 tests/projects/svghmi_pathslider/plc.xml
beremiz 69eba82c20c865f74b0582d6e33f43ed8c3e8ce90b47ede51b36cb670bba497a tests/projects/svghmi_real/plc.xml
beremiz 2e25b6fd05c7ec75e9bc8dca38c3c12bec208235253c20e98f7e90b0d4d69a47 tests/projects/svghmi_scrollbar/plc.xml
beremiz 6b2885c163b035181b2a99cb7d980c58aba23a64125d639505c906f20fe90cc5 tests/projects/svghmi_widgets/plc.xml
beremiz e084f857907a1bbc161f15c87de8c27b2431ae994a2d973548a63c7a3e8fcb45 tests/projects/svghmi_xy/plc.xml
beremiz 703714bbb580b05a1a941f22c13276a807d2d133d49e82e1cfa079a0a223f9a0 tests/projects/wamp/plc.xml
beremiz 2fed93929b9358681b21ceacd517bce2e3e3480e9740d66fc2db564641e2fa0d tests/projects/wiimote/plc.xml
beremiz e985dd836d187c7ee4e0e559e42ba6f99991219329c464d02655cc00bf40f410 tests/projects/wxGlade/plc.xml
beremiz 9e9b5d47caaa0b44fbe75e757e6d45f96ae585f393713510d37a6b077440ae34 tests/projects/wxHMI/plc.xml
openplc b54083621492188567f8020af4514a565f4f91c3b8e1b3741ccd11672abdb043 examples/Arduino_7_Segments/plc.xml
openplc c7d5b21f5a1b70ddddc057fdec539b68f8eeff9e3c8924c7b16d201234ceee26 examples/Arduino_Cloud/plc.xml
openplc 5ef1d515710616843af107ea1a5be7baf574fbd4d3d71dc367403454cf8c4891 examples/Arduino_LCD_Screen/plc.xml
openplc 268a0793234239ae90abd5009ee5b4fa1e4cf1b3aebc1c8df4b9eeb7a0be3d2e examples/Arduino_Serial_Print/plc.xml
openplc 38b18912864678019d34cf2623dfa2cd7f9bf916a214e09d82a0a6c16a6805d1 examples/Blink/plc.xml
openplc 16aaff019a9b8316d584631f19cd31e964c7491b04302c76781c80bc9f56f00f examples/Blink_P1AM/plc.xml
openplc 43b5ca83392960bfce06d50bcecf777cc754209d6cf8b6dee44198f13198b85f examples/MQTT_Send/plc.xml
openplc 74bf73e0dda3efb91c83c8901ee382c364b2122ab40de5297a1543be7c77e989 examples/MQTT_Send_Receive/plc.xml
openplc 8c2d996344782ac16a3b84a88fa685579b4f991f09cdad5ed0df8d9137a18b58 examples/Multi_Language/plc.xml
openplc 5a9db3467177da67e8611dd2daa74ff86c8649aa4bbb0f222fd65b4df14cceb7 examples/Random_Generator_Pragma/plc.xml
openplc 376f2eaa5f55c708aef6c2434647d37c32153af5bb855228a8cd27b845e97814 examples/STM32CAN_Read_Write_FBD/plc.xml
openplc 23f1427c276a8437bf50fcec49a4b0ec1685c101d362c5f6a35dd8b4228e015d examples/TCP_socket/plc.xml
openplc 2d42be053d6fd9509c45d7a72ce5ea4802361c318216f527c18128f54779060d examples/Traffic_Light_FBD/plc.xml
openplc 6ca1278088ac0ba77cb259efaaf586490b18ef6b2fc677603fce45f3dbb1d550 exemples/csv_read/plc.xml
openplc 2260185b09c1a6c597bbbc98c937955b572f7cd0e05799b0dcc885c8864abc76 exemples/first_steps/plc.xml
openplc 71edc38dc7c32c62d2163dcc171d406ad3079cb9b04fafc216d5a0868c507adc exemples/modbus/plc.xml
openplc 7449692ab85475d9a79273b5d34b2798e1a9259b144a4521878ad5e1ee47ebdc exemples/python/plc.xml
openplc f20845be5efc353f6931406e32fdc3ed1f9e0a38d915830e7e6c10471b12b310 exemples/svghmi_jumps/plc.xml
openplc cc1ada616e4b23b6d14bccbd9e156bc6c1eb0ec161ff426fb1b521054f186443 exemples/svghmi_references/plc.xml
openplc f7e22a67fa51cabd0884de1232f2c3b4f02de7902b0e32e6f1584e710d86c375 exemples/svghmi_traffic_light/plc.xml
openplc c34bfde3652082988d3ef234fdf1028e356bed638504d50e4a576f61520b3715 tests/projects/BACnet/plc.xml
openplc 2c8b42363393ac1f84bc1761e293e80b88ab6fd3b144f8f539ba7c0f47cbe3a7 tests/projects/canopen_master/plc.xml
openplc 9809e4da432ddeddb221ee058dd794fef1ff9dcc523b7db764b70d7098b28e88 tests/projects/canopen_slave/plc.xml
openplc 9a6ac7df13aa616d41b637c588f14f08c079b13ea61670aa51c8bb3ae80b736f tests/projects/genericmake/plc.xml
openplc 01176e157bebd0ed367b7cdade5e3f5dab9d59cab43d1ac24be2dd75c7ea4814 tests/projects/iec61131_lang_test/plc.xml
openplc 00dbfa8a087e83cb3188398e490bbb246457b221e054a9b087dd5875439a3daf tests/projects/logging/plc.xml
openplc 32ca94bfe3e9da84e939d6d56a4f5db0734ebb35fbf58fd0ed6f59f718177630 tests/projects/opcua_browse/plc.xml
openplc 32ca94bfe3e9da84e939d6d56a4f5db0734ebb35fbf58fd0ed6f59f718177630 tests/projects/opcua_browse_encrypted/plc.xml
openplc 99a92822bf26de1d0db17ee5dec1fc6dd9cc34136db42aff638c58bb82fd58f5 tests/projects/opcua_client/plc.xml
openplc 1b6f5bdb22371770df25ae33799579d74a63cf1f7ddc52fef30fa2e90840d6d8 tests/projects/opcua_client_encrypted/plc.xml
openplc 3ee7bdbc8bbaf0b6c46d008ad31150342f434949ec2cee8b2264c41b1d44de1a tests/projects/svghmi/plc.xml
openplc ab088fe38b7c1baa06a20d6942f69ba817417b81092358cda15dfc82bc517ec4 tests/projects/svghmi_basic/plc.xml
openplc dae8c0054ee7902045715791bda13d578d879ec10bd899a0d0c1bf42f98176a2 tests/projects/svghmi_i18n/plc.xml
openplc a353ed6e672e43e26ae1c1c68f023f4d13bba559c855ad3436a3c8870f1e3d68 tests/projects/svghmi_pathslider/plc.xml
openplc 69eba82c20c865f74b0582d6e33f43ed8c3e8ce90b47ede51b36cb670bba497a tests/projects/svghmi_real/plc.xml
openplc 2e25b6fd05c7ec75e9bc8dca38c3c12bec208235253c20e98f7e90b0d4d69a47 tests/projects/svghmi_scrollbar/plc.xml
openplc f98d3c4eabb19c161870a0126d79dc85df402f917cae9d9877c22eea238adc8a tests/projects/svghmi_v2/plc.xml
openplc 6b2885c163b035181b2a99cb7d980c58aba23a64125d639505c906f20fe90cc5 tests/projects/svghmi_widgets/plc.xml
openplc e084f857907a1bbc161f15c87de8c27b2431ae994a2d973548a63c7a3e8fcb45 tests/projects/svghmi_xy/plc.xml
openplc 703714bbb580b05a1a941f22c13276a807d2d133d49e82e1cfa079a0a223f9a0 tests/projects/wamp/plc.xml
openplc 2fed93929b9358681b21ceacd517bce2e3e3480e9740d66fc2db564641e2fa0d tests/projects/wiimote/plc.xml
openplc e985dd836d187c7ee4e0e559e42ba6f99991219329c464d02655cc00bf40f410 tests/projects/wxGlade/plc.xml
openplc 9e9b5d47caaa0b44fbe75e757e6d45f96ae585f393713510d37a6b077440ae34 tests/projects/wxHMI/plc.xml
"

fetched=0
while read -r name want path; do
  [ -n "$name" ] || continue
  dest="$corpus/$name/$path"
  if [ -f "$dest" ] && [ "$(sha256 "$dest")" = "$want" ]; then
    continue
  fi
  mkdir -p "$(dirname "$dest")"
  curl -fsSL --retry 3 -o "$dest.part" "$(url_of "$name")/$path"
  got="$(sha256 "$dest.part")"
  if [ "$got" != "$want" ]; then
    rm -f "$dest.part"
    echo "SHA-256 mismatch for $name/$path: got $got, want $want" >&2
    exit 1
  fi
  mv "$dest.part" "$dest"
  fetched=$((fetched + 1))
done <<<"$manifest"
echo "corpus: $corpus ($fetched fetched)"

HELMSMAN_PLCOPEN_CORPUS="$corpus" cargo test --locked -p helmsman-iec --test plcopen_corpus -- --nocapture
