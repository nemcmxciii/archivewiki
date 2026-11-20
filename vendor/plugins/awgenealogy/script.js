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
});
