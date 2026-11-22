/* ArchiveWiki Genealogy: fill in or edit a person right on the chart (no pop-up dialogs). */
jQuery(function () {
    var $ = jQuery;

    function field(name, label, value) {
        return $('<input type="text" class="edit"/>').attr({name: name, placeholder: label, title: label}).val(value || '');
    }

    function openForm($box, mode, vals) {
        if ($box.data('awg-open')) return;
        $box.data('awg-open', true);
        var $old = $box.children().not('.awg-num').detach();
        var $f = $('<div class="awg-form"/>');
        var $name = field('name', 'Full name', vals.name);
        var $born = field('born', 'Born (date)', vals.born);
        var $bp = field('birthplace', 'Birthplace', vals.birthplace);
        var $died = field('died', 'Died (date)', vals.died);
        var $dp = field('deathplace', 'Place of death', vals.deathplace);
        var $err = $('<div class="awg-err"/>');
        var $save = $('<button type="button" class="button"/>').text('Save');
        var $cancel = $('<button type="button" class="button"/>').text('Cancel');
        $f.append($name, $('<div class="awg-row"/>').append($born, $bp), $('<div class="awg-row"/>').append($died, $dp), $err, $save, $cancel);
        $box.append($f);
        $box.closest('.awg-chart').css('overflow', 'visible');
        $name.focus();

        function close() {
            $f.remove();
            $box.append($old);
            $box.data('awg-open', false);
        }
        $cancel.on('click', close);
        $f.on('keydown', function (e) { if (e.which === 27) close(); if (e.which === 13) $save.click(); });
        $save.on('click', function () {
            $err.text('');
            if ($.trim($name.val()) === '') { $err.text('Please enter a name.'); return; }
            var data = {
                call: 'awgenealogy',
                mode: mode,
                id: mode === 'add' ? vals.child : vals.id,
                sectok: $box.closest('.awg-chart').data('sectok'),
                name: $name.val(), born: $born.val(), birthplace: $bp.val(), died: $died.val(), deathplace: $dp.val()
            };
            if (mode === 'add') data.role = vals.role;
            $save.prop('disabled', true);
            $.post(DOKU_BASE + 'lib/exe/ajax.php', data, null, 'json')
                .done(function (r) {
                    if (r && r.ok) { window.location.reload(); }
                    else { $err.text((r && r.message) || 'Could not save.'); $save.prop('disabled', false); }
                })
                .fail(function (xhr) { $err.text('Could not save (' + xhr.status + ').'); $save.prop('disabled', false); });
        });
    }

    $(document).on('click', '.awg-chart .awg-add', function (e) {
        e.preventDefault();
        var $box = $(this).closest('.awg-box');
        openForm($box, 'add', {child: $box.data('child'), role: $box.data('role')});
    });
    $(document).on('click', '.awg-chart .awg-edit', function (e) {
        e.preventDefault();
        var $box = $(this).closest('.awg-box');
        openForm($box, 'update', {
            id: $box.data('id'), name: $box.attr('data-name'), born: $box.attr('data-born'),
            birthplace: $box.attr('data-birthplace'), died: $box.attr('data-died'), deathplace: $box.attr('data-deathplace')
        });
    });

    // ---------------------------------------------------------------- fillable form view
    $('.awg-formwrap').each(function () {
        var $w = $(this);
        var $form = $w.find('form.awg-form');
        var $save = $form.find('.awg-save');
        var $msg = $form.find('.awg-msg');
        var $dirty = $form.find('.awg-dirty');

        function card(n) { return $form.find('.awg-card[data-n="' + n + '"]'); }
        function nameOf($c) { return $.trim($c.find('input[name$="[name]"]').val() || ''); }
        function hasPerson($c) { return !!$c.attr('data-id') || nameOf($c) !== ''; }

        // A card unlocks once the person below it (its child on the chart) has a name.
        function refresh() {
            $form.find('.awg-card').each(function () {
                var $c = $(this), n = parseInt($c.attr('data-n'), 10);
                if (n === 1 || $c.attr('data-id')) { return; }
                var $p = card(Math.floor(n / 2));
                var open = hasPerson($p) && !$p.hasClass('awg-blocked');
                $c.toggleClass('awg-blocked', !open);
                $c.find('input').prop('disabled', !open);
                $c.find('.awg-hint').text(open ? 'Not created yet' : 'Fill in the previous generation first.');
            });
        }
        refresh();

        $form.on('input change', 'input', function () {
            $save.prop('disabled', false);
            $dirty.text('Unsaved changes');
            $msg.removeClass('awg-bad awg-good').text('');
            refresh();
        });

        $form.on('submit', function (e) {
            e.preventDefault();
            $form.find('.awg-err').remove();
            $form.find('.awg-card').removeClass('awg-has-error');
            $msg.removeClass('awg-bad awg-good').text('');
            var data = $form.serializeArray();
            data.push(
                {name: 'call', value: 'awgenealogy'}, {name: 'mode', value: 'saveform'},
                {name: 'id', value: $w.attr('data-root')}, {name: 'gens', value: $w.attr('data-gens')},
                {name: 'sectok', value: $w.attr('data-sectok')}
            );
            $save.prop('disabled', true);
            $dirty.text('Saving…');
            $.post(DOKU_BASE + 'lib/exe/ajax.php', data, null, 'json')
                .done(function (r) {
                    if (r && r.ok) {
                        $msg.addClass('awg-good').text(r.message);
                        window.location.reload();
                        return;
                    }
                    $msg.addClass('awg-bad').text((r && r.message) || 'Could not save.');
                    var first = null;
                    $.each((r && r.errors) || {}, function (n, text) {
                        var $c = card(n);
                        $c.addClass('awg-has-error').append($('<div class="awg-err"/>').text(text));
                        if (!first) { first = $c; }
                    });
                    if (first && first[0] && first[0].scrollIntoView) { first[0].scrollIntoView({block: 'center'}); }
                    $save.prop('disabled', false);
                    $dirty.text('Unsaved changes');
                })
                .fail(function (xhr) {
                    $msg.addClass('awg-bad').text('Could not save (' + xhr.status + ').');
                    $save.prop('disabled', false);
                    $dirty.text('Unsaved changes');
                });
        });
    });
});
