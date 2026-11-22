<?php

use dokuwiki\Extension\ActionPlugin;
use dokuwiki\Extension\EventHandler;
use dokuwiki\Extension\Event;

/**
 * AJAX endpoint for creating person pages and adding or editing family details.
 * @license GPL 2 (http://www.gnu.org/licenses/gpl.html)
 */
class action_plugin_awgenealogy extends ActionPlugin
{
    public function register(EventHandler $controller)
    {
        $controller->register_hook('AJAX_CALL_UNKNOWN', 'BEFORE', $this, 'handleAjax');
    }

    private function reply($ok, $msg, array $extra = [])
    {
        header('Content-Type: application/json; charset=utf-8');
        echo json_encode(array_merge(['ok' => $ok, 'message' => $msg], $extra));
    }

    public function handleAjax(Event $event, $param)
    {
        if ($event->data !== 'awgenealogy') return;
        $event->stopPropagation();
        $event->preventDefault();

        global $INPUT;
        /** @var helper_plugin_awgenealogy $h */
        $h = plugin_load('helper', 'awgenealogy');

        if (!checkSecurityToken()) return $this->reply(false, 'Security token expired. Reload the page and try again.');

        $mode = $INPUT->post->str('mode');
        $id = cleanID($INPUT->post->str('id'));
        if ($mode === 'newperson') {
            if ($id !== 'genealogy:start' || !page_exists($id)) return $this->reply(false, 'The Genealogy page is unavailable.');
            return $this->createPerson($h);
        }
        if ($mode === 'newfamily') {
            if ($id !== 'genealogy:families:start' || !page_exists($id)) return $this->reply(false, 'The Family Groups page is unavailable.');
            return $this->saveFamily($h, '', true);
        }
        if ($mode === 'savefamily') {
            if (strpos($id, 'genealogy:families:') !== 0 || noNS($id) === 'start') return $this->reply(false, 'That family group page is invalid.');
            if (!page_exists($id)) return $this->reply(false, 'That family group no longer exists.');
            if (auth_quickaclcheck($id) < AUTH_EDIT) return $this->reply(false, 'You don\'t have permission to edit this family group.');
            if (checklock($id) !== false) return $this->reply(false, 'The family group is being edited by someone else.');
            return $this->saveFamily($h, $id, false);
        }
        if ($id === '' || !page_exists($id)) return $this->reply(false, 'That person page no longer exists.');
        if (auth_quickaclcheck($id) < AUTH_EDIT) return $this->reply(false, 'You don\'t have permission to edit this page.');
        if (checklock($id) !== false) return $this->reply(false, 'The page is being edited by someone else.');

        $values = [];
        foreach (helper_plugin_awgenealogy::EDITABLE as $k) {
            if ($INPUT->post->has($k)) $values[$k] = $h->cleanValue($INPUT->post->str($k));
        }

        if ($mode === 'saveform') return $this->saveForm($h, $id);

        if ($mode === 'update') {
            if (array_key_exists('name', $values) && $values['name'] === '') return $this->reply(false, 'A name is required.');
            if (!empty($values['died'])) $values['living'] = 'no';
            $text = $h->setFields(rawWiki($id), $values);
            saveWikiText($id, $text, 'Details edited from the pedigree chart');
            return $this->reply(true, 'Saved.');
        }

        if ($mode === 'add') {
            $role = $INPUT->post->str('role');
            if (!in_array($role, ['father', 'mother'], true)) return $this->reply(false, 'Unknown role.');
            $name = $values['name'] ?? '';
            if ($name === '') return $this->reply(false, 'A name is required.');

            $ns = getNS($id);
            $base = cleanID(($ns !== '' ? $ns . ':' : '') . $name);
            if ($base === '') return $this->reply(false, 'That name can\'t be used for a page.');
            $new = $base;
            for ($i = 2; page_exists($new); $i++) $new = $base . '_' . $i;
            if (auth_quickaclcheck($new) < AUTH_CREATE) return $this->reply(false, 'You don\'t have permission to create that page.');

            $fields = $values;
            unset($fields['name']);
            $fields['living'] = empty($values['died']) ? 'yes' : 'no';
            $fields['sex'] = $role === 'father' ? 'M' : 'F';
            saveWikiText($new, $h->newPersonText($new, $name, $fields + ['name' => $name]), 'Added from the pedigree chart');

            $ref = (getNS($new) === $ns) ? noNS($new) : $new;
            saveWikiText($id, $h->setFields(rawWiki($id), [$role => $ref]), 'Linked ' . $role . ' from the pedigree chart');
            return $this->reply(true, 'Added.', ['id' => $new]);
        }

        return $this->reply(false, 'Unknown action.');
    }

    /** Create a person's page from the short Genealogy landing-page form. */
    private function createPerson($h)
    {
        global $INPUT;
        $name = $h->cleanValue($INPUT->post->str('name'), 120);
        if ($name === '') return $this->reply(false, 'Enter a name to create the person page.');

        $values = [];
        foreach (['born', 'birthplace', 'died', 'deathplace'] as $key) {
            $values[$key] = $h->cleanValue($INPUT->post->str($key));
        }
        $baseName = str_replace(':', ' ', $name);
        $base = cleanID('genealogy:' . $baseName);
        $new = $base;
        for ($i = 2; page_exists($new); $i++) $new = $base . '_' . $i;
        if (auth_quickaclcheck($new) < AUTH_CREATE) return $this->reply(false, 'You don\'t have permission to create a person page here.');

        $values['living'] = $values['died'] === '' ? 'yes' : 'no';
        $text = $h->newPersonText($new, $name, $values);
        saveWikiText($new, $text, 'Created a person page');
        return $this->reply(true, 'Person page created.', ['url' => wl($new, '', false, '&')]);
    }

    private function familyMember($h, array $input, $role, array &$reserved)
    {
        $name = $h->cleanValue($input['name'] ?? '', 120);
        $explicitId = cleanID($input['id'] ?? '');
        if ($name === '' && $explicitId === '') {
            foreach (['born', 'birthplace', 'died', 'deathplace', 'sex'] as $key) {
                if ($h->cleanValue($input[$key] ?? '') !== '') return ['error' => 'Enter a name for the person or clear the other fields.'];
            }
            return null;
        }
        if ($name === '') return ['error' => 'Enter a name for each linked person.'];

        $fields = [
            'name' => $name,
            'born' => $h->cleanValue($input['born'] ?? ''),
            'birthplace' => $h->cleanValue($input['birthplace'] ?? ''),
            'died' => $h->cleanValue($input['died'] ?? ''),
            'deathplace' => $h->cleanValue($input['deathplace'] ?? ''),
        ];
        if ($role === 'child') $fields['sex'] = $h->cleanValue($input['sex'] ?? '', 20);
        $id = $explicitId;
        $existing = false;
        if ($id !== '') {
            if (strpos($id, 'genealogy:') !== 0 || !page_exists($id) || !$h->findBlock(rawWiki($id))) {
                return ['error' => 'A linked person page could not be found.'];
            }
            if (auth_quickaclcheck($id) < AUTH_EDIT || checklock($id) !== false) {
                return ['error' => 'A linked person page cannot be edited right now.'];
            }
            $existing = true;
        } else {
            $base = cleanID('genealogy:' . str_replace(':', ' ', $name));
            $id = $base;
            if (page_exists($id) && $h->findBlock(rawWiki($id))) {
                $existing = true;
                if (auth_quickaclcheck($id) < AUTH_EDIT || checklock($id) !== false) {
                    return ['error' => 'A matching person page cannot be edited right now.'];
                }
            } else {
                for ($i = 2; page_exists($id) || isset($reserved[$id]); $i++) $id = $base . '_' . $i;
                if (auth_quickaclcheck($id) < AUTH_CREATE) return ['error' => 'You don\'t have permission to create a person page here.'];
            }
        }
        if (isset($reserved[$id])) return ['error' => 'The same person was entered more than once in this family group.'];
        $reserved[$id] = true;
        return ['id' => $id, 'name' => $name, 'fields' => $fields, 'existing' => $existing, 'explicit' => $explicitId !== ''];
    }

    private function saveFamily($h, $familyId, $isNew)
    {
        global $INPUT;
        $title = $h->cleanValue($INPUT->post->str('title'), 120);
        if ($title === '') return $this->reply(false, 'Enter a name for this family group.');

        if ($isNew) {
            $base = cleanID('genealogy:families:' . str_replace(':', ' ', $title));
            $familyId = $base;
            for ($i = 2; page_exists($familyId); $i++) $familyId = $base . '_' . $i;
            if (auth_quickaclcheck($familyId) < AUTH_CREATE) return $this->reply(false, 'You don\'t have permission to create a family group here.');
        }

        $reserved = [];
        $members = [];
        foreach (['father', 'mother'] as $role) {
            $input = $INPUT->post->arr($role);
            $member = $this->familyMember($h, is_array($input) ? $input : [], $role, $reserved);
            if (isset($member['error'])) return $this->reply(false, $member['error']);
            if ($member) $members[$role] = $member;
        }
        $children = [];
        $childrenIn = $INPUT->post->arr('children');
        if (is_array($childrenIn)) {
            if (count($childrenIn) > 20) return $this->reply(false, 'A family group can include up to 20 children.');
            foreach ($childrenIn as $input) {
                if (!is_array($input)) continue;
                $child = $this->familyMember($h, $input, 'child', $reserved);
                if (isset($child['error'])) return $this->reply(false, $child['error']);
                if ($child) $children[] = $child;
            }
        }
        $familyFields = [
            'title' => $title,
            'father' => $members['father']['id'] ?? '',
            'mother' => $members['mother']['id'] ?? '',
            'marriage_date' => $h->cleanValue($INPUT->post->str('marriage_date')),
            'marriage_place' => $h->cleanValue($INPUT->post->str('marriage_place')),
            'sources' => $this->cleanFamilyText($h, $INPUT->post->str('sources')),
            'notes' => $this->cleanFamilyText($h, $INPUT->post->str('notes')),
        ];
        foreach ($children as $i => $child) $familyFields['child_' . ($i + 1)] = $child['id'];

        foreach (array_merge(array_values($members), $children) as $person) {
            if ($person['existing']) {
                $changes = ['name' => $person['name']];
                foreach (['born', 'birthplace', 'died', 'deathplace', 'sex'] as $key) {
                    if (isset($person['fields'][$key]) && ($person['explicit'] || $person['fields'][$key] !== '')) {
                        $changes[$key] = $person['fields'][$key];
                    }
                }
                if (($changes['died'] ?? '') !== '') $changes['living'] = 'no';
                saveWikiText($person['id'], $h->setFields(rawWiki($person['id']), $changes), 'Updated from a family group');
            } else {
                $fields = $person['fields'];
                if ($fields['sex'] === '') unset($fields['sex']);
                $fields['living'] = $fields['died'] === '' ? 'yes' : 'no';
                saveWikiText($person['id'], $h->newPersonText($person['id'], $person['name'], $fields), 'Added from a family group');
            }
        }

        foreach ($children as $child) {
            $links = [];
            foreach (['father', 'mother'] as $role) {
                if (!isset($members[$role])) continue;
                $parentId = $members[$role]['id'];
                $links[$role] = getNS($parentId) === getNS($child['id']) ? noNS($parentId) : $parentId;
            }
            if ($links) saveWikiText($child['id'], $h->setFields(rawWiki($child['id']), $links), 'Linked from a family group');
        }

        if ($isNew) saveWikiText($familyId, $h->newFamilyText($familyFields), 'Created a family group');
        else saveWikiText($familyId, $h->setFamilyFields(rawWiki($familyId), $familyFields), 'Updated a family group');
        return $this->reply(true, $isNew ? 'Family group created.' : 'Family group saved.', ['url' => wl($familyId, '', false, '&')]);
    }

    private function cleanFamilyText($h, $value)
    {
        $lines = preg_split('/\r\n|\r|\n/', (string)$value);
        $lines = array_map(function ($line) use ($h) { return $h->cleanValue($line, 500); }, $lines);
        $clean = implode('\n', $lines);
        return function_exists('mb_substr') ? mb_substr($clean, 0, 2000) : substr($clean, 0, 2000);
    }

    private function livingFor(array $vals, array $in)
    {
        if (($vals['died'] ?? '') !== '') return 'no';
        if (preg_match('/\b(1[0-9]{3}|20[0-9]{2})\b/', $vals['born'] ?? '', $m) && (int)$m[1] <= (int)date('Y') - 110) return 'no';
        return (($in['living'] ?? '') === '1') ? 'yes' : 'no';
    }

    /** Save every card of the pedigree form in one go: validate everything first, then apply. */
    private function saveForm($h, $rootId)
    {
        global $INPUT;
        $gens = max(2, min(6, $INPUT->post->int('gens', 5)));
        $posted = $INPUT->post->arr('p');
        $map = $h->ancestors($rootId, $gens);
        if ($map === null) return $this->reply(false, 'The chart\'s person page no longer exists.');

        $ops = [];
        $errors = [];
        $slotId = [];
        $reserved = [];
        $total = (1 << $gens) - 1;
        for ($n = 1; $n <= $total; $n++) {
            $cur = $map[$n] ?? null;
            $in = (isset($posted[$n]) && is_array($posted[$n])) ? $posted[$n] : null;
            $vals = [];
            foreach (helper_plugin_awgenealogy::EDITABLE as $k) $vals[$k] = $h->cleanValue($in[$k] ?? '');

            if (is_array($cur) && isset($cur['person']) && empty($cur['person']['private'])) {
                $pid = $cur['id'];
                $slotId[$n] = $pid;
                if ($in === null) continue;
                $f = $cur['person']['fields'];
                $changes = [];
                foreach (helper_plugin_awgenealogy::EDITABLE as $k) {
                    if (trim($f[$k] ?? '') !== $vals[$k]) $changes[$k] = $vals[$k];
                }
                $living = $this->livingFor($vals, $in);
                $stored = in_array(strtolower(trim($f['living'] ?? '')), ['yes', 'y', 'true', '1'], true) ? 'yes' : 'no';
                if ($living !== $stored) $changes['living'] = $living;
                if (!$changes) continue;
                if (array_key_exists('name', $changes) && $changes['name'] === '') {
                    $errors[$n] = 'A name is required (to remove someone, delete their page).';
                    continue;
                }
                if (auth_quickaclcheck($pid) < AUTH_EDIT) { $errors[$n] = 'You don\'t have permission to edit this page.'; continue; }
                if (checklock($pid) !== false) { $errors[$n] = 'This page is being edited by someone else.'; continue; }
                $ops[] = ['type' => 'update', 'id' => $pid, 'changes' => $changes];
                continue;
            }
            if (is_array($cur) && isset($cur['person'])) continue; // private: leave alone

            // empty slot: create a person if anything was typed
            if ($in === null) continue;
            if (!array_filter($vals)) continue;
            if ($vals['name'] === '') { $errors[$n] = 'Please enter a name.'; continue; }
            $childN = intdiv($n, 2);
            if (!isset($slotId[$childN])) { $errors[$n] = 'Fill in the previous generation first.'; continue; }
            $childId = $slotId[$childN];
            $ns = getNS($childId);
            $base = cleanID(($ns !== '' ? $ns . ':' : '') . $vals['name']);
            if ($base === '') { $errors[$n] = 'That name can\'t be used for a page.'; continue; }
            $new = $base;
            for ($i = 2; page_exists($new) || isset($reserved[$new]); $i++) $new = $base . '_' . $i;
            if (auth_quickaclcheck($new) < AUTH_CREATE) { $errors[$n] = 'You don\'t have permission to create this page.'; continue; }
            if (page_exists($childId) && (auth_quickaclcheck($childId) < AUTH_EDIT || checklock($childId) !== false)) {
                $errors[$n] = 'The previous generation\'s page can\'t be edited right now.';
                continue;
            }
            $reserved[$new] = true;
            $slotId[$n] = $new;
            $fields = $vals;
            unset($fields['name']);
            $fields['living'] = $this->livingFor($vals, $in);
            $fields['sex'] = ($n % 2 === 0) ? 'M' : 'F';
            $ops[] = ['type' => 'create', 'id' => $new, 'name' => $vals['name'], 'fields' => $fields,
                      'child' => $childId, 'role' => ($n % 2 === 0) ? 'father' : 'mother'];
        }

        if ($errors) return $this->reply(false, 'Please fix the highlighted people and save again. Nothing was saved.', ['errors' => $errors]);
        if (!$ops) return $this->reply(true, 'Nothing to save.', ['created' => 0, 'updated' => 0]);

        $created = 0;
        $updated = 0;
        foreach ($ops as $op) {
            if ($op['type'] === 'update') {
                saveWikiText($op['id'], $h->setFields(rawWiki($op['id']), $op['changes']), 'Edited from the pedigree form');
                $updated++;
            } else {
                saveWikiText($op['id'], $h->newPersonText($op['id'], $op['name'], $op['fields'] + ['name' => $op['name']]), 'Added from the pedigree form');
                $ns = getNS($op['child']);
                $ref = (getNS($op['id']) === $ns) ? noNS($op['id']) : $op['id'];
                saveWikiText($op['child'], $h->setFields(rawWiki($op['child']), [$op['role'] => $ref]), 'Linked ' . $op['role'] . ' from the pedigree form');
                $created++;
            }
        }
        return $this->reply(true, 'Saved.', ['created' => $created, 'updated' => $updated]);
    }
}
