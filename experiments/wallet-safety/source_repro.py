#!/usr/bin/env python3
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest

LANE = pathlib.Path(__file__).resolve().parent
ROOT = LANE.parent.parent / "parano1d"
EXPECTED_PIN = "52938433a30b185ab007afaa15166a632cf2e82e"


def source(relative):
    return (ROOT / relative).read_text(encoding="utf-8")


def block(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class PinnedSourceRepro(unittest.TestCase):
    def test_revision_is_the_requested_pin(self):
        actual = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip()
        self.assertEqual(actual, EXPECTED_PIN)

    def test_gui_submits_without_an_exact_fee_review(self):
        app = source("noid_gui/src/app.rs")
        submit = block(app, "Message::SubmitSend =>", "Message::ProofForgeTick")
        self.assertIn(".send_payment(recipient, amount_micronoid, fee_micronoid)", submit)
        self.assertNotIn("plan_send", submit)
        self.assertNotIn("walletPlanSend", submit)

        backend = source("noid_gui/src/backend.rs")
        send = block(backend, "pub async fn send_payment", "pub async fn plan_consolidation")
        self.assertIn('"walletSend"', send)
        self.assertNotIn('"walletPlanSend"', send)

    def test_gui_success_recipient_is_not_recovered_from_the_signed_intent(self):
        backend = source("noid_gui/src/backend.rs")
        send = block(backend, "pub async fn send_payment", "pub async fn plan_consolidation")
        construction = send.rsplit("Ok(PaymentSubmission {", 1)[1].split("})", 1)[0]
        self.assertIn("recipient,", construction)
        self.assertNotIn("result.recipient", construction)
        self.assertIn("amount_micronoid: result.amount_micronoid", construction)

        rpc_types = source("noid_rpc/src/types.rs")
        result = block(rpc_types, "pub struct WalletSendResult {", "}")
        self.assertNotIn("recipient", result)
        self.assertNotIn("intent", result)

    def test_rpc_divergence_gate_checks_only_fee_and_counts(self):
        server = source("noid_rpc/src/server.rs")
        condition = block(
            server,
            "if actual_fee != fee_micronoid",
            "let reservation = match PendingAdmissionGuard::reserve",
        )
        self.assertIn("input_count != expected_input_count", condition)
        self.assertIn("output_count != expected_output_count", condition)
        self.assertNotIn("to_address", condition)
        self.assertNotIn("outputs", condition)
        response = block(server, "return Ok(WalletSendResult {", "});")
        self.assertIn("amount_micronoid,", response)
        self.assertNotIn("signed", response)

    def test_native_flow_has_the_review_binding_missing_from_ordinary_send(self):
        operations = source("noid_gui/src/assets/operations.rs")
        finish = block(operations, "pub fn finish_submit", "pub fn clear")
        self.assertIn("submission.plan_id != preview.plan_id", finish)
        self.assertIn("submission.txid != preview.txid", finish)
        self.assertIn("submission.fee_micronoid != preview.fee_micronoid", finish)

    def test_export_clears_gui_memory_but_not_the_system_clipboard(self):
        app = source("noid_gui/src/app.rs")
        copy = block(app, "Message::CopyExportedSecret =>", "Message::BeginImportSecret")
        close = block(app, "Message::CloseSecretDialog =>", "Message::ImportSecretFinished")
        self.assertIn("iced::clipboard::write", copy)
        self.assertIn("self.exported_master_secret.clear()", close)
        self.assertNotIn("clipboard::write", close)


@unittest.skipUnless(os.environ.get("PARANO1D_BIN"), "set PARANO1D_BIN to run CLI repros")
class SyntheticCliKeystoreRepro(unittest.TestCase):
    def test_owner_only_key_export_and_symlink_rejection(self):
        secret = "51" + "".join(f"{(index * 13) % 256:02x}" for index in range(1, 32))
        self.assertEqual(len(secret), 64)
        with tempfile.TemporaryDirectory(dir=LANE / "target") as temporary:
            root = pathlib.Path(temporary)
            data = root / "data"
            config = root / "config.toml"
            binary = os.environ["PARANO1D_BIN"]

            def maintenance(operation, stdin=None):
                return subprocess.run(
                    [
                        binary,
                        "--config",
                        str(config),
                        "--data-dir",
                        str(data),
                        "--log",
                        "error",
                        operation,
                    ],
                    input=stdin,
                    text=True,
                    capture_output=True,
                    timeout=60,
                    check=False,
                )

            imported = maintenance("--import-wallet-secret", secret + "\n")
            self.assertEqual(imported.returncode, 0, imported.stderr)
            key = data / "wallet.key"
            self.assertEqual(stat.S_IMODE(key.stat().st_mode), 0o600)
            self.assertEqual(len(key.read_bytes()), 48)
            exported = maintenance("--export-wallet-secret")
            self.assertEqual(exported.returncode, 0, exported.stderr)
            self.assertEqual(exported.stdout.strip(), secret)

            held = root / "held-synthetic.key"
            key.rename(held)
            key.symlink_to(held)
            rejected = maintenance("--export-wallet-secret")
            self.assertNotEqual(rejected.returncode, 0)
            self.assertNotIn(secret, rejected.stdout + rejected.stderr)


if __name__ == "__main__":
    (LANE / "target").mkdir(exist_ok=True)
    unittest.main(verbosity=2)
