#!/usr/bin/env python3
"""GTK may publish a control's name before its action interface is ready."""
import unittest
from unittest.mock import Mock, patch
import gui_smoke as s

class ActionExportTests(unittest.TestCase):
    def test_late_action_export_waits_then_activates_exactly_once(self):
        node=object();action=Mock()
        action.get_n_actions.return_value=1
        action.get_action_name.return_value='click'
        with patch.object(s,'find_all',return_value=[node]), patch.object(s,'press_actions',side_effect=[(None,[]),(None,[]),(action,['click'])]):
            self.assertIs(s.click_named(None,'Exit fullscreen'),node)
        action.do_action.assert_called_once_with(0)
    def test_a_missing_action_still_fails_without_clicking(self):
        with patch.object(s,'find_all',return_value=[]),patch.object(s,'TIMEOUT',0.01),patch.object(s,'POLL',0.001):
            with self.assertRaises(s.Failure):s.click_named(None,'Absent control')
if __name__=='__main__':unittest.main()
