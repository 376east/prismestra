import os
import subprocess
import sys
import unittest
from unittest.mock import Mock, call, patch

from capture_console_windows import WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT, WindowCapture, window_observation
from normalize_process_events import normalize
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


def event(kind, pid, time, image, parent=0, session=1):
    return {'kind': kind, 'filetime': str(time), 'pid': pid, 'parent_pid': parent,
            'session_id': session, 'image': image}


def capture():
    return {
        'format': 'prismestra-windows-process-events-v1',
        'started_filetime': '100',
        'ended_filetime': '200',
        'harness_pid': 1,
        'before_control': {'pid': 9, 'start_filetime': '90', 'stop_filetime': '91'},
        'after_control': {'pid': 9, 'start_filetime': '201', 'stop_filetime': '202'},
        'events': [
            event('stop', 9, 202, 'cmd.exe'),
            event('stop', 2, 190, 'Setup.exe'),
            event('start', 9, 90, 'cmd.exe', 1),
            event('start', 2, 110, 'Setup.exe', 1),
            event('start', 9, 201, 'cmd.exe', 1),
            event('stop', 9, 91, 'cmd.exe'),
        ],
    }


class NormalizeProcessEventsTests(unittest.TestCase):
    def normalize(self, data, roots=('2:110',)):
        return normalize(data, 'setup', REVISION, ARTIFACT, roots)

    def test_control_shells_are_outside_the_scenario_and_windows_are_uncovered(self):
        trace = self.normalize(capture())
        self.assertEqual([process['id'] for process in trace['processes']], ['2:110'])
        self.assertTrue(trace['controls']['process_start_stop'])
        self.assertFalse(trace['controls']['visible_console'])
        self.assertEqual(check_trace(trace, REVISION, ARTIFACT)['status'], 'inconclusive')

    def test_pids_are_resolved_at_creation_across_start_stop_and_reuse(self):
        data = capture()
        data['events'].extend([
            event('start', 4, 115, 'background.exe', 1),
            event('stop', 4, 120, 'background.exe'),
            event('start', 4, 130, 'SetupWorker.exe', 2),
            event('start', 5, 135, 'powershell.exe', 4),
            event('stop', 5, 137, 'powershell.exe'),
            event('stop', 4, 150, 'SetupWorker.exe'),
        ])
        trace = self.normalize(data)
        process = next(row for row in trace['processes'] if row['id'] == '5:135')
        self.assertEqual(process['parent'], '4:130')
        self.assertEqual(check_trace(trace, REVISION, ARTIFACT)['violations'],
                         ['Product started powershell.exe (5:135)'])

    def test_inactive_or_simultaneous_parent_leaves_an_explicit_gap(self):
        for parent_stop in (120, 130):
            data = capture()
            data['events'].extend([
                event('start', 4, 115, 'worker.exe', 2),
                event('stop', 4, parent_stop, 'worker.exe'),
                event('start', 5, 130, 'cmd.exe', 4),
            ])
            trace = self.normalize(data)
            self.assertFalse(trace['controls']['process_start_stop'])
            self.assertIsNone(trace['processes'][-1]['parent'])
            self.assertEqual(check_trace(trace, REVISION, ARTIFACT)['status'], 'inconclusive')

    def test_missing_stop_control_or_root_rejects_receipt(self):
        data = capture()
        with self.assertRaisesRegex(ValueError, 'declared root'):
            self.normalize(data, roots=('2:111',))
        data['events'].pop(0)
        with self.assertRaisesRegex(ValueError, 'matching command'):
            self.normalize(data)


class ConsoleWindowCaptureTests(unittest.TestCase):
    def test_records_both_console_classes_without_guessing_an_unresolved_owner(self):
        for class_name in ('ConsoleWindowClass', 'CASCADIA_HOSTING_WINDOW_CLASS'):
            row = window_observation(50, class_name, 0, 'show', 123)
            self.assertEqual(row['class'], class_name)
            self.assertIsNone(row['window_owner_pid'])
            self.assertEqual(row['event_tick'], 123)
        self.assertIsNone(window_observation(50, 'PrismestraMainWindow', 70, 'initial'))
        observed = window_observation(50, 'ConsoleWindowClass', 70, 'initial', owner_start='123')
        self.assertEqual(observed['window_owner_created_filetime'], '123')

    def test_hook_filters_accessibility_objects_and_reports_observer_errors(self):
        observer = WindowCapture.__new__(WindowCapture)
        observer.gaps = []
        observer.observe = Mock()
        observer.on_show(None, 0, 50, -4, 0, 7, 100)
        observer.on_show(None, 0, 50, 0, 1, 7, 100)
        observer.observe.assert_not_called()
        observer.on_show(None, 0, 50, 0, 0, 7, 100)
        observer.observe.assert_called_once_with(50, 'show', 100)
        observer.observe.side_effect = OSError('synthetic window access failure')
        observer.on_show(None, 0, 50, 0, 0, 7, 100)
        self.assertIn('synthetic window access failure', observer.gaps[0])


class ProcessExitCaptureTests(unittest.TestCase):
    image = r'C:\Test\Prismestra.exe'

    def fake_observer(self, creation=108, image=None, wait=(WAIT_TIMEOUT, WAIT_OBJECT_0)):
        observer = WindowCapture.__new__(WindowCapture)
        observer.open_process = Mock(return_value=42)
        observer.close_handle = Mock()
        observer.handle_pid = Mock(return_value=2)
        observer.wait_process = Mock(side_effect=wait)

        ticks = iter((120, 145))

        def clock_tick(target):
            target._obj.dwLowDateTime = next(ticks)

        observer.precise_utc = Mock(side_effect=clock_tick)

        def copy_image(_handle, _flags, name, _size):
            name.value = image or self.image
            return True

        def copy_times(_handle, created, exited, _kernel, _user):
            created._obj.dwLowDateTime = creation
            exited._obj.dwLowDateTime = 140
            return True

        observer.handle_image = Mock(side_effect=copy_image)
        observer.process_times = Mock(side_effect=copy_times)
        return observer

    def test_waits_on_the_same_live_handle_and_does_not_fill_a_missing_stop(self):
        observer = self.fake_observer()
        armed = Mock()
        # A coarse Python clock returning 139 would precede the kernel exit at
        # 140 even though the same live handle signaled. Bypass that clock.
        with patch('capture_console_windows.utc_filetime', side_effect=('120', '139')) as clock:
            receipt = observer.watch_process_exit(2, '108', self.image, 3, armed)
        clock.assert_not_called()
        self.assertEqual(observer.precise_utc.call_count, 2)
        armed.assert_called_once_with()
        observer.open_process.assert_called_once()
        observer.close_handle.assert_called_once_with(42)
        self.assertEqual(observer.wait_process.call_args_list, [call(42, 0), call(42, 3000)])
        self.assertEqual([entry.args[0] for entry in observer.process_times.call_args_list], [42, 42])
        self.assertEqual(receipt['exit_filetime'], '140')

        data = capture()
        data['events'].remove(event('stop', 2, 190, 'Setup.exe'))
        data['events'].append(event('start', 2, 150, 'setup2.exe', 1))
        # The independent Win32 creation time is 108. The WMI start event is 110.
        launch = {'pid': 2, 'created_filetime': '108', 'image': self.image.lower()}
        trace = normalize(data, 'setup', REVISION, ARTIFACT, ['2:110'],
                          process_exit_receipt=receipt, expected_exit=launch)
        self.assertEqual(trace['identified_process_exit'], receipt)
        self.assertEqual(trace['process_capture_gaps'], ['PID 2 was reused without a captured stop'])
        self.assertFalse(trace['controls']['process_start_stop'])
        self.assertEqual(check_trace(trace, REVISION, ARTIFACT)['status'], 'inconclusive')
        receipt['expected']['created_filetime'] = '150'
        with self.assertRaisesRegex(ValueError, 'match the expected identity'):
            normalize(data, 'setup', REVISION, ARTIFACT, ['2:110'],
                      process_exit_receipt=receipt, expected_exit=launch)

    def test_internally_valid_exit_requires_the_independent_launch_identity(self):
        launch = {'pid': 2, 'created_filetime': '108', 'image': self.image}
        for unrelated in (
            dict(launch, pid=3),
            dict(launch, created_filetime='150'),
            dict(launch, image=r'C:\Other\Prismestra.exe'),
        ):
            with self.subTest(unrelated=unrelated):
                receipt = {
                    'format': 'prismestra-windows-process-exit-v1', 'status': 'confirmed',
                    'expected': unrelated.copy(), 'observed': unrelated.copy(),
                    'armed_filetime': '160', 'exit_filetime': '170', 'confirmed_filetime': '180',
                }
                with self.assertRaisesRegex(ValueError, 'separate expected launch identity'):
                    normalize(capture(), 'setup', REVISION, ARTIFACT, ['2:110'],
                              process_exit_receipt=receipt)
                with self.assertRaisesRegex(ValueError, 'current launch identity'):
                    normalize(capture(), 'setup', REVISION, ARTIFACT, ['2:110'],
                              process_exit_receipt=receipt, expected_exit=launch)

    def test_reused_pid_and_unverified_image_never_arm(self):
        for observer in (self.fake_observer(creation=109),
                         self.fake_observer(image=r'C:\Other\Prismestra.exe')):
            armed = Mock()
            with self.assertRaisesRegex(RuntimeError, 'match the expected identity'):
                observer.watch_process_exit(2, '108', self.image, 3, armed)
            armed.assert_not_called()
            observer.wait_process.assert_not_called()
            observer.close_handle.assert_called_once_with(42)

    def test_failed_identity_query_cannot_confirm(self):
        for operation in ('handle_image', 'process_times'):
            with self.subTest(operation=operation):
                observer = self.fake_observer()
                getattr(observer, operation).side_effect = None
                getattr(observer, operation).return_value = False
                armed = Mock()
                with patch('ctypes.get_last_error', return_value=5, create=True), patch(
                    'ctypes.WinError', side_effect=lambda code: OSError(code), create=True
                ), self.assertRaises(OSError):
                    observer.watch_process_exit(2, '108', self.image, 3, armed)
                armed.assert_not_called()
                observer.wait_process.assert_not_called()
                observer.close_handle.assert_called_once_with(42)

    def test_already_dead_wait_failure_timeout_and_open_denial_cannot_confirm(self):
        for waits, error, armed_count in [
            ((WAIT_OBJECT_0,), RuntimeError, 0),
            ((WAIT_TIMEOUT, WAIT_TIMEOUT), RuntimeError, 1),
            ((WAIT_TIMEOUT, WAIT_FAILED), OSError, 1),
        ]:
            with self.subTest(waits=waits):
                observer = self.fake_observer(wait=waits)
                armed = Mock()
                with patch('ctypes.get_last_error', return_value=5, create=True), patch(
                    'ctypes.WinError', side_effect=lambda code: OSError(code), create=True
                ), self.assertRaises(error):
                    observer.watch_process_exit(2, '108', self.image, 3, armed)
                self.assertEqual(armed.call_count, armed_count)
                observer.close_handle.assert_called_once_with(42)
        observer = self.fake_observer()
        observer.open_process.return_value = None
        armed = Mock()
        with patch('ctypes.get_last_error', return_value=5, create=True), patch(
            'ctypes.WinError', side_effect=lambda code: OSError(code), create=True
        ), self.assertRaises(OSError):
            observer.watch_process_exit(2, '108', self.image, 3, armed)
        armed.assert_not_called()
        observer.wait_process.assert_not_called()
        observer.close_handle.assert_not_called()

    @unittest.skipUnless(os.name == 'nt', 'requires Win32 process handles')
    def test_native_wait_on_own_child(self):
        observer = WindowCapture()
        with subprocess.Popen([sys.executable, '-c', 'import sys; sys.stdin.read()'],
                              stdin=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW) as child:
            try:
                created = observer.owner_created_filetime(child.pid)
                self.assertIsNotNone(created)
                receipt = observer.watch_process_exit(child.pid, created, sys.executable, 5,
                                                      child.stdin.close)
                self.assertEqual(receipt['observed']['pid'], child.pid)
                self.assertEqual(receipt['observed']['created_filetime'], created)
                self.assertEqual(child.wait(timeout=5), 0)
            finally:
                child.stdin.close()


if __name__ == '__main__':
    unittest.main()
