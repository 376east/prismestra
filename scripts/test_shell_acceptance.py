import unittest

from shell_acceptance import check_trace


REVISION = 'a' * 40
ARTIFACT = 'b' * 64


def trace():
    return {
        'scenario': 'legacy-migration',
        'revision': REVISION,
        'artifact_sha256': ARTIFACT,
        'controls': {'process_start_stop': True, 'visible_console': True, 'full_scenario': True},
        'product_roots': ['2:20', '5:50'],
        'harness_roots': ['1:10'],
        'processes': [
            {'id': '1:10', 'parent': None, 'image': 'pwsh.exe'},
            {'id': '2:20', 'parent': '1:10', 'image': 'Prismestra.exe'},
            {'id': '3:30', 'parent': '1:10', 'image': 'cmd.exe'},
            {'id': '5:50', 'parent': None, 'image': 'PrismestraHelper.exe'},
        ],
        'windows': [{'process': '3:30', 'class': 'ConsoleWindowClass', 'visible': True}],
    }


class ShellAcceptanceTests(unittest.TestCase):
    def test_harness_shell_is_outside_product_even_when_it_starts_the_product(self):
        self.assertEqual(check_trace(trace(), REVISION, ARTIFACT)['status'], 'pass')

    def test_reused_pid_keeps_the_product_shell_separate_from_harness_generation(self):
        data = trace()
        data['processes'].append({'id': '3:60', 'parent': '5:50', 'image': 'C:/Windows/cmd.EXE'})
        result = check_trace(data, REVISION, ARTIFACT)
        self.assertEqual(result['status'], 'fail')
        self.assertEqual(result['violations'], ['Product started cmd.exe (3:60)'])

    def test_product_console_fails_without_a_shell_executable(self):
        data = trace()
        data['windows'].append({'process': '2:20', 'class': 'ConsoleWindowClass', 'visible': True})
        self.assertEqual(check_trace(data, REVISION, ARTIFACT)['status'], 'fail')

    def test_detached_console_without_an_owner_does_not_pass(self):
        data = trace()
        data['windows'].append({
            'process': None, 'class': 'CASCADIA_HOSTING_WINDOW_CLASS', 'visible': True,
        })
        self.assertEqual(check_trace(data, REVISION, ARTIFACT)['status'], 'inconclusive')

    def test_missing_control_root_or_expected_revision_does_not_pass(self):
        data = trace()
        data['controls']['visible_console'] = False
        data['processes'].pop()
        result = check_trace(data, 'c' * 40, ARTIFACT)
        self.assertEqual(result['status'], 'inconclusive')
        self.assertEqual(len(result['gaps']), 3)

    def test_harness_label_cannot_hide_a_descendant_of_the_product(self):
        data = trace()
        data['processes'].append({'id': '6:60', 'parent': '5:50', 'image': 'bash.exe'})
        data['harness_roots'].append('6:60')
        self.assertEqual(check_trace(data, REVISION, ARTIFACT)['status'], 'fail')

    def test_shell_in_a_different_build_does_not_decide_the_requested_build(self):
        data = trace()
        data['processes'].append({'id': '6:60', 'parent': '5:50', 'image': 'pwsh.exe'})
        self.assertEqual(check_trace(data, 'c' * 40, ARTIFACT)['status'], 'inconclusive')

    def test_setup_receipt_cannot_pass_for_uninstall(self):
        data = trace()
        data['scenario'] = 'setup'
        result = check_trace(data, REVISION, ARTIFACT, 'uninstall')
        self.assertEqual(result['status'], 'inconclusive')
        self.assertEqual(result['gaps'], ['Trace scenario differs from the requested scenario'])

    def test_unresolved_harness_ancestor_cannot_hide_a_shell(self):
        data = trace()
        data['processes'][0]['parent'] = '7:7'
        with self.assertRaisesRegex(ValueError, 'Unresolved parent'):
            check_trace(data, REVISION, ARTIFACT)

    def test_future_and_same_pid_parents_are_invalid(self):
        for parent in ['1:100', '3:1']:
            with self.subTest(parent=parent):
                data = trace()
                data['processes'].append({'id': parent, 'parent': None, 'image': 'helper.exe'})
                data['processes'][2]['parent'] = parent
                with self.assertRaisesRegex(ValueError, 'Impossible parent'):
                    check_trace(data, REVISION, ARTIFACT)

    def test_acyclic_links_with_the_same_start_time_are_valid(self):
        data = trace()
        data['processes'].append({'id': '4:30', 'parent': '1:10', 'image': 'helper.exe'})
        data['processes'][2]['parent'] = '4:30'
        self.assertEqual(check_trace(data, REVISION, ARTIFACT)['status'], 'pass')

    def test_old_harness_generation_cannot_own_child_after_pid_is_reused(self):
        data = trace()
        data['processes'].extend([
            {'id': '1:60', 'parent': '5:50', 'image': 'helper.exe'},
            {'id': '6:70', 'parent': '1:10', 'image': 'powershell.exe'},
        ])
        with self.assertRaisesRegex(ValueError, 'Superseded parent'):
            check_trace(data, REVISION, ARTIFACT)
        data['processes'][-1]['parent'] = '1:60'
        self.assertEqual(check_trace(data, REVISION, ARTIFACT)['status'], 'fail')

    def test_equal_timestamp_cycle_is_invalid_even_without_shells(self):
        data = trace()
        data['processes'].extend([
            {'id': '6:60', 'parent': '7:60', 'image': 'worker-a.exe'},
            {'id': '7:60', 'parent': '6:60', 'image': 'worker-b.exe'},
        ])
        with self.assertRaisesRegex(ValueError, 'Process ancestry cycle'):
            check_trace(data, REVISION, ARTIFACT)


if __name__ == '__main__':
    unittest.main()
