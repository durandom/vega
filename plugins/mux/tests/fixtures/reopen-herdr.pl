#!/usr/bin/perl
# Fake herdr for reopen::tests. Perl, not Python: the test makes ~260 herdr calls, and a
# Python start (behind macOS's xcrun stub) cost about 5x a Perl one.
use strict;
use warnings;
use Cwd qw(abs_path);
use JSON::PP;

my $json = JSON::PP->new->allow_nonref->canonical;
my @args = @ARGV;
my $root = $ENV{XDG_STATE_HOME};
my $kind = $ENV{VEGA_TEST_KIND};
my $live = "$root/running";

sub slurp {
    my ($path) = @_;
    open(my $fh, '<', $path) or die "read $path: $!";
    local $/;
    my $text = <$fh>;
    return $text;
}
sub spew {
    my ($path, $text) = @_;
    open(my $fh, '>', $path) or die "write $path: $!";
    print $fh $text;
    close($fh) or die "write $path: $!";
}
sub emit { print $json->encode({ result => $_[0] }), "\n"; exit 0 }
sub fail {
    print $json->encode({ error => { code => $_[0], message => 'fixture failure' } }), "\n";
    exit 1;
}
sub key_of { (my $k = $_[0]) =~ s/:/_/g; "local_$k" }
sub arg_after {
    my ($flag) = @_;
    for my $i (0 .. $#args - 1) { return $args[$i + 1] if $args[$i] eq $flag }
    die "no $flag in @args";
}

my $agent = -e $live ? $json->decode(slurp($live)) : undef;
my $cmd = join(' ', @args[0 .. 1]);

if ($cmd eq 'agent get') {
    fail('agent_not_found')
      unless $agent && ($args[2] eq $agent->{name} || $args[2] eq $agent->{pane_id});
    emit({ agent => $agent });
} elsif ($cmd eq 'agent list') {
    emit({ agents => $agent ? [$agent] : [] });
} elsif ($cmd eq 'pane get') {
    fail('pane_not_found') unless $agent && $args[2] eq $agent->{pane_id};
    delete $agent->{agent_session}
      if $ENV{VEGA_TEST_RESUME} eq '0' && $ENV{VEGA_TEST_DIALOG} eq '0' && !-e "$root/prompted";
    emit({ pane => $agent });
} elsif ($cmd eq 'pane list') {
    emit({ panes => [{ pane_id => 'w0:p1', workspace_id => 'w0', agent => 'codex' }] });
} elsif ($cmd eq 'pane process-info') {
    # The caller's pane runs the test. A spawned pane has no shell: spawn renices its
    # shell, and reniced, the test and all its fixture calls would crawl on a loaded host.
    my $caller = $args[3] eq 'w0:p1';
    emit({ process_info => { shell_pid => $caller ? $ENV{VEGA_TEST_PID} + 0 : 0 } });
} elsif ($cmd eq 'pane read') {
    print "Ready\n";
} elsif ($cmd eq 'workspace get') {
    emit({ workspace => {} });
} elsif ($cmd eq 'pane close') {
    my $key = key_of($args[2]);
    die "parent of $args[2] left" if -e "$root/vega/parents/$key";
    die "mode of $args[2] left" if -e "$root/vega/modes/$key";
    unlink $live;
    emit({});
} elsif ($cmd eq 'agent prompt') {
    spew("$root/prompted", '');
    emit({});
} elsif ($cmd eq 'agent wait') {
    emit({});
} elsif ($cmd eq 'workspace create') {
    die 'workspace cwd is not the test dir' unless abs_path(arg_after('--cwd')) eq abs_path($root);
    my $pane = $ENV{VEGA_TEST_NEXT_PANE};
    my ($ws) = split(/:/, $pane);
    emit({ workspace => { workspace_id => $ws }, root_pane => { pane_id => $pane } });
} elsif ($cmd eq 'agent start') {
    my $pane = arg_after('--pane');
    my $key = key_of($pane);
    my $expected = $json->decode($ENV{VEGA_TEST_PARENT});
    my $parent_file = "$root/vega/parents/$key";
    if ($expected) {
        my $parent = $json->decode(slurp($parent_file));
        die 'wrong parent'
          unless $parent->{parent} eq $expected->{pane} && $parent->{host} eq $expected->{host};
    } else {
        die 'unexpected parent' if -e $parent_file;
    }
    my $mode = $json->decode(slurp("$root/vega/modes/$key"));
    die "wrong mode $mode->{mode}" unless $mode->{mode} eq $ENV{VEGA_TEST_MODE};
    my ($dashes) = grep { $args[$_] eq '--' } 0 .. $#args;
    my @tail = @args[$dashes + 1 .. $#args];
    if ($ENV{VEGA_TEST_RESUME} eq '1') {
        my $flag = $kind eq 'codex' ? 'resume' : '--resume';
        die "no resume in @tail" unless @tail >= 2 && $tail[-2] eq $flag && $tail[-1] eq 'saved-session';
    } else {
        die "unexpected resume in @tail" if grep { $_ eq 'resume' || $_ eq '--resume' } @tail;
    }
    my ($ws) = split(/:/, $pane);
    spew($live, $json->encode({
        name => 'fix', pane_id => $pane, workspace_id => $ws, agent => $kind,
        agent_session => { value => 'saved-session' },
    }));
    fail('timeout') if $ENV{VEGA_TEST_DIALOG} eq '1';
    emit({});
} else {
    die "unexpected call: @args";
}
