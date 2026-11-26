program ft4_stage4_dump
  use packjt77
  include 'ft4_params.f90'

  integer, parameter :: NDMAX = NMAX / NDOWN        ! 4032
  integer, parameter :: NSS = NSPS / NDOWN          ! 32
  integer, parameter :: NFRAME = NN * NSS           ! 3296
  integer, parameter :: NBITS = 2 * NN              ! 206
  integer, parameter :: MAXCAND = 200
  real, parameter :: SYNCMIN = 1.18
  real, parameter :: SCALEFAC = 2.83

  character(len=512) :: in_path, out_dir, arg
  character(len=13) :: mycall_arg, hiscall_arg, mycall0, hiscall0
  character(len=37) :: msg, msg_send
  character(len=77) :: c77
  character(len=40) :: decoded_msgs(MAXCAND)
  integer :: fa_i, fb_i, nfqso, ndepth, max_samples, ncontest, nQSOProgress
  real :: fa, fb, twopi, fs2, f0, f1, smax, smax1, smax_c, sum2, sum2_cb, sync, dt, xdt, snr, xsnr, qual, apmag, dmin
  integer :: i, j, k, isp, ic, icand, ncand, iseg, ib, idf, ibest, idfbest, ibest_c, idfbest_c
  integer :: nsync_qual, nsym_ok, npasses, ipass, iaptype, maxosd, ndeep, nharderror, ntype
  integer :: napwid, ndec, ndec_total, did_sub_first, np, ilen, nohiscall_i4, badsync_i4
  integer :: i3, n3, i1, i2, bad_i4
  logical :: newdat, badsync, nomycall, nohiscall, unpk77_success, is_dup
  integer :: npatience, nthreads
  common/patience/npatience,nthreads

  integer*2 :: iwave(NMAX)
  real :: dd(NMAX), savg(NH1), savg_lin(NH1), sbase(NH1), x(NFFT1), win(NFFT1)
  real :: candidate(2, MAXCAND)
  complex :: cx(0:NH1), cd2(0:NDMAX-1), cb(0:NDMAX-1), cd(0:NFRAME-1)
  complex :: ctwk2(2*NSS, -16:16), c0_vec(2*NSS), cs(0:3, NN), csymb(NSS)
  real :: bitmetrics(NBITS, 3), llra(174), llrb(174), llrc(174), llrd(174), apbits(174)
  real :: s4(0:3, NN), s1(0:3)
  integer*1 :: message91(91), message77(77), cw(174), apmask(174)
  integer*1 :: mcq(29), mcqtest(29), mcqfd(29), mcqrru(29), mcqru(29), mcqww(29), mru(29)
  integer*1 :: apmy_ru(28), aphis_fd(28), msgbits(77)
  integer :: rvec(77), itone(NN), itmp(ND)
  integer :: icos4a(0:3), icos4b(0:3), icos4c(0:3), icos4d(0:3), graymap(0:3)
  integer :: nappasses(0:5), naptypes(0:5, 3)

  data icos4a/0,1,3,2/
  data icos4b/1,0,2,3/
  data icos4c/2,3,1,0/
  data icos4d/3,2,0,1/
  data graymap/0,1,3,2/
  data nappasses/2,2,2,2,2,3/
  data rvec/0,1,0,0,1,0,1,0,0,1,0,1,1,1,1,0,1,0,0,0,1,0,0,1,1,0,1,1,0, &
            1,0,0,1,0,1,1,0,0,0,0,1,0,0,0,1,0,1,0,0,1,1,1,1,0,0,1,0,1, &
            0,1,0,1,0,1,1,0,1,1,1,1,1,0,0,0,1,0,1/

  naptypes(0,:) = (/1,2,0/)
  naptypes(1,:) = (/2,3,0/)
  naptypes(2,:) = (/2,3,0/)
  naptypes(3,:) = (/3,6,0/)
  naptypes(4,:) = (/3,6,0/)
  naptypes(5,:) = (/3,1,2/)

  npatience = 0
  nthreads = 1

  if (command_argument_count() .lt. 11) then
     write(0,*) 'usage: ft4_stage4_dump <raw_i16.bin> <out_dir> <fa> <fb> ', &
          '<nfqso> <ndepth> <max_samples> <mycall> <hiscall> <ncontest> <qso_progress>'
     stop 1
  endif

  call get_command_argument(1, in_path)
  call get_command_argument(2, out_dir)
  call get_command_argument(3, arg); read(arg, *) fa
  call get_command_argument(4, arg); read(arg, *) fb
  call get_command_argument(5, arg); read(arg, *) nfqso
  call get_command_argument(6, arg); read(arg, *) ndepth
  call get_command_argument(7, arg); read(arg, *) max_samples
  call get_command_argument(8, mycall_arg)
  call get_command_argument(9, hiscall_arg)
  call get_command_argument(10, arg); read(arg, *) ncontest
  call get_command_argument(11, arg); read(arg, *) nQSOProgress

  if (trim(mycall_arg) .eq. '-') mycall_arg = ''
  if (trim(hiscall_arg) .eq. '-') hiscall_arg = ''
  if (max_samples .gt. NMAX) max_samples = NMAX
  if (max_samples .lt. 0) max_samples = 0

  mycall13 = mycall_arg
  dxcall13 = hiscall_arg

  iwave = 0
  open(unit=10, file=trim(in_path), access='stream', form='unformatted', status='old')
  read(10) iwave
  close(10)

  dd = 0.0
  do i = 1, max_samples
     dd(i) = real(iwave(i))
  enddo

  ! Precompute frequency-tweak vectors ctwk2(:, -16:16) identically to ft4_decode.f90
  twopi = 8.0 * atan(1.0)
  fs2 = 12000.0 / real(NDOWN)
  c0_vec = cmplx(1.0, 0.0)
  do idf = -16, 16
     call twkfreq1(c0_vec, 2*NSS, fs2/2.0, (/real(idf), 0.0, 0.0, 0.0, 0.0/), ctwk2(:, idf))
  enddo

  ! Build AP prefix masks matching ft4_decode.f90
  mcq    = 2* (/0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0/) - 1
  mcqtest= 2* (/0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,1,1,0,0,0,0,1,0,0,1,0,1,0/) - 1
  mcqfd  = 2* (/0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,1,0,0,0,0,0,0,0/) - 1
  mcqru  = 2* (/0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,1,0,0,1,0,1,0,0,0,1,1,0/) - 1
  mcqww  = 2* (/0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,1,0,1,1,0,0,1,1,0,1,0,0,0/) - 1
  ! Re-pack static CQs directly via pack77 and XOR with rvec to verify bit equivalence:
  call build_cq_prefix('CQ K1ABC FN42', mcq, rvec)
  call build_cq_prefix('CQ TEST K1ABC FN42', mcqtest, rvec)
  call build_cq_prefix('CQ FD K1ABC FN42', mcqfd, rvec)
  call build_cq_prefix('CQ RU K1ABC FN42', mcqrru, rvec)
  call build_cq_prefix('CQ WW K1ABC FN42', mcqww, rvec)

  mycall0 = mycall_arg
  hiscall0 = hiscall_arg
  nomycall = (len_trim(mycall0) .lt. 3)
  nohiscall = (len_trim(hiscall0) .lt. 3)
  if (nomycall) mycall0 = 'K1ABC'
  if (nohiscall) hiscall0 = mycall0

  msg = trim(mycall0)//' '//trim(hiscall0)//' RR73'
  i3 = -1; n3 = -1
  call pack77(msg, i3, n3, c77)
  do i = 1, 77
     read(c77(i:i), '(I1)') msgbits(i)
  enddo
  do i = 1, 28
     apmy_ru(i) = ieor(msgbits(i), int(rvec(i+1), 1))
     aphis_fd(i) = ieor(msgbits(29+i), int(rvec(28+i), 1))
  enddo
  do i = 1, 77
     msgbits(i) = ieor(msgbits(i), int(rvec(i), 1))
  enddo
  call encode174_91(msgbits, cw)
  apbits = 2.0 * real(cw) - 1.0
  if (nohiscall) then
     apbits(30) = 99.0
     nohiscall_i4 = 1
  else
     nohiscall_i4 = 0
  endif

  open(unit=11, file=trim(out_dir)//'/ap_set.bin', access='stream', form='unformatted', status='replace')
  write(11) nohiscall_i4, apbits, apmy_ru, aphis_fd
  close(11)

  open(unit=15, file=trim(out_dir)//'/decoded.txt', status='replace')
  ndec_total = 0
  did_sub_first = 0

  do isp = 1, 3
     if (isp .eq. 1) then
        call compute_savg_linear(dd, savg_lin)
     endif

     savg = 0.0; sbase = 0.0; candidate = 0.0; ncand = 0
     call getcandidates4(dd, fa, fb, SYNCMIN, nfqso, MAXCAND, savg, candidate, ncand, sbase)

     if (isp .eq. 1) then
        open(unit=12, file=trim(out_dir)//'/candidates.bin', access='stream', form='unformatted', status='replace')
        write(12) int(ncand, 4), savg_lin, sbase
        if (ncand .gt. 0) write(12) candidate(1:2, 1:ncand)
        close(12)

        open(unit=13, file=trim(out_dir)//'/cands_cd2.bin', access='stream', form='unformatted', status='replace')
        open(unit=17, file=trim(out_dir)//'/eval_bitmetrics.bin', access='stream', form='unformatted', status='replace')
        open(unit=18, file=trim(out_dir)//'/ldpc_wins.bin', access='stream', form='unformatted', status='replace')
     endif

     if (ncand .eq. 0) then
        if (isp .eq. 1) then
           close(13); close(17); close(18)
        endif
        exit
     endif

     ndec = 0
     do ic = 1, ncand
        f0 = candidate(1, ic)
        snr = candidate(2, ic) - 1.0
        newdat = (ic .eq. 1)
        call ft4_downsample(dd, newdat, f0, cd2)
        sum2 = sum(abs(cd2)**2) / real(NDMAX)
        if (sum2 .gt. 0.0) cd2 = cd2 / sqrt(sum2)

        if (isp .eq. 1) then
           write(13) int(ic, 4), f0, snr, cd2
        endif

        smax1 = 0.0
        do iseg = 1, 3
           if (iseg .eq. 1) then
              i1 = 108; i2 = 560
           elseif (iseg .eq. 2) then
              smax1 = smax
              i1 = 560; i2 = 1012
           else
              i1 = -344; i2 = 108
           endif

           smax_c = -99.0
           ibest_c = i1; idfbest_c = 0
           do idf = -12, 12, 3
              do ib = i1, i2, 4
                 call sync4d(cd2, ib, ctwk2(:, idf), 1, sync)
                 if (sync .gt. smax_c) then
                    smax_c = sync
                    ibest_c = ib
                    idfbest_c = idf
                 endif
              enddo
           enddo

           smax = -99.0
           ibest = ibest_c; idfbest = idfbest_c
           do idf = idfbest_c - 4, idfbest_c + 4
              do ib = ibest_c - 5, ibest_c + 5
                 call sync4d(cd2, ib, ctwk2(:, idf), 1, sync)
                 if (sync .gt. smax) then
                    smax = sync
                    ibest = ib
                    idfbest = idf
                 endif
              enddo
           enddo

           if (iseg .eq. 1) then
              smax1 = smax
              if (smax .lt. 1.2) cycle
           else
              if (smax .lt. smax1 .or. smax .lt. 1.2) cycle
           endif

           f1 = f0 + real(idfbest)
           if (f1 .le. 10.0 .or. f1 .ge. 4990.0) cycle

           call ft4_downsample(dd, .false., f1, cb)
           sum2_cb = sum(abs(cb)**2) / real(NFRAME)
           if (sum2_cb .gt. 0.0) cb = cb / sqrt(sum2_cb)

           cd = cmplx(0.0, 0.0)
           if (ibest .ge. 0) then
              np = min(NDMAX - 1, ibest + NFRAME - 1) - ibest + 1
              if (np .gt. 0) cd(0:np-1) = cb(ibest:ibest+np-1)
           else
              ilen = NFRAME + 2*ibest
              if (ilen .gt. 0) cd(-ibest:ibest+NFRAME-1) = cb(0:NFRAME-1+2*ibest)
           endif

           badsync = .false.
           bitmetrics = 0.0
           call get_ft4_bitmetrics(cd, bitmetrics, badsync)
           if (badsync) cycle

           call count_sync_qual_ft4(bitmetrics, nsync_qual)
           if (nsync_qual .lt. 20) cycle

           call extract_ft4_payload_llrs(bitmetrics, llra, llrb, llrc)
           if (isp .eq. 1) then
              write(17) int(ic, 4), int(iseg, 4), smax, int(ibest, 4), int(idfbest, 4), f1, &
                   int(nsync_qual, 4), cb, cd, bitmetrics, llra, llrb, llrc
           endif

           ndeep = 2
           napwid = 50
           if (ndepth .le. 2) then
              maxosd = -1
           elseif (abs(nfqso - nint(f1)) .le. napwid) then
              maxosd = 3
           else
              maxosd = 2
           endif

           npasses = 3
           if (ndepth .ge. 2) npasses = 3 + nappasses(min(max(nQSOProgress,0),5))

           nharderror = -1
           do ipass = 1, npasses
              apmask = 0
              if (ipass .eq. 1) then
                 iaptype = 0; llrd = llra
              elseif (ipass .eq. 2) then
                 iaptype = 0; llrd = llrb
              elseif (ipass .eq. 3) then
                 iaptype = 0; llrd = llrc
              else
                 iaptype = naptypes(min(max(nQSOProgress,0),5), ipass - 3)
                 llrd = llrc
                 apmag = maxval(abs(llra)) * 1.1
                 if (iaptype .eq. 1) then
                    apmask(1:29) = 1
                    call apply_cq_llr(ncontest, apmag, mcq, mcqtest, mcqfd, mcqrru, mcqww, llrd)
                 elseif (iaptype .eq. 2) then
                    if (nomycall) cycle
                    if (ncontest .eq. 4) then
                       apmask(2:29) = 1
                       llrd(2:29) = apmag * (2.0 * real(apmy_ru) - 1.0)
                    else
                       apmask(1:28) = 1
                       llrd(1:28) = apmag * apbits(1:28)
                    endif
                 elseif (iaptype .eq. 3) then
                    if (nomycall .or. nohiscall .or. abs(nfqso - f1) .gt. napwid) cycle
                    if (ncontest .eq. 3) then
                       apmask(1:28) = 1; apmask(29:56) = 1
                       llrd(1:28) = apmag * apbits(1:28)
                       llrd(29:56) = apmag * (2.0 * real(aphis_fd) - 1.0)
                    elseif (ncontest .eq. 4) then
                       apmask(2:29) = 1; apmask(30:57) = 1
                       llrd(2:29) = apmag * (2.0 * real(apmy_ru) - 1.0)
                       llrd(30:57) = apmag * apbits(30:57)
                    else
                       apmask(1:58) = 1
                       llrd(1:58) = apmag * apbits(1:58)
                    endif
                 elseif (iaptype .eq. 6) then
                    if (nomycall .or. nohiscall .or. abs(nfqso - f1) .gt. napwid) cycle
                    apmask(1:77) = 1
                    llrd(1:77) = apmag * apbits(1:77)
                 endif
              endif

              cw = 0; message91 = 0; ntype = 0; nharderror = -1; dmin = 0.0
              call decode174_91(llrd, 91, maxosd, ndeep, apmask, message91, cw, ntype, nharderror, dmin)

              if (nharderror .ge. 0) then
                 message77 = message91(1:77)
                 if (sum(int(message77)) .eq. 0) then
                    nharderror = -1
                    cycle
                 endif
                 do i = 1, 77
                    message77(i) = ieor(message77(i), int(rvec(i), 1))
                    write(c77(i:i), '(I1)') message77(i)
                 enddo
                 unpk77_success = .false.
                 msg = ''
                 call unpack77(c77, 1, msg, unpk77_success)
                 if (.not. unpk77_success) then
                    exit
                 endif

                 ! Recompute channel tones from CW
                 do i = 1, ND
                    itmp(i) = graymap(2 * int(cw(2*i-1)) + int(cw(2*i)))
                 enddo
                 itone(1:4) = icos4a
                 itone(5:33) = itmp(1:29)
                 itone(34:37) = icos4b
                 itone(38:66) = itmp(30:58)
                 itone(67:70) = icos4c
                 itone(71:99) = itmp(59:87)
                 itone(100:103) = icos4d

                 dt = real(ibest) / 666.67
                 call subtractft4(dd, itone, f1, dt)

                 if (isp .eq. 1) then
                    write(18) int(ic, 4), int(iseg, 4), int(ipass, 4), int(iaptype, 4), int(maxosd, 4), &
                              int(nharderror, 4), dmin, apmask, llrd, cw, message77
                 endif

                 if (did_sub_first .eq. 0) then
                    did_sub_first = 1
                    open(unit=14, file=trim(out_dir)//'/sub_first.bin', access='stream', form='unformatted', status='replace')
                    write(14) int(ic, 4), f1, dt, itone, dd
                    close(14)
                 endif

                 is_dup = .false.
                 do j = 1, ndec_total
                    if (decoded_msgs(j) .eq. trim(msg)) is_dup = .true.
                 enddo
                 if (.not. is_dup) then
                    ndec = ndec + 1
                    ndec_total = ndec_total + 1
                    decoded_msgs(ndec_total) = trim(msg)
                    xdt = dt - 0.5
                    xsnr = -21.0
                    if (snr .gt. 0.0) xsnr = max(-21.0, 10.0 * log10(snr) - 14.8)
                    qual = 1.0 - (real(nharderror) + dmin) / 60.0
                    write(15, '(I2,1X,I4,1X,F6.2,1X,I5,1X,I2,1X,F7.4,1X,I3,1X,F8.3,1X,A)') &
                         isp, nint(xsnr), xdt, nint(f1), iaptype, qual, nharderror, dmin, trim(msg)
                 endif
                 exit
              endif
           enddo

           if (nharderror .ge. 0) exit
        enddo
     enddo

     if (isp .eq. 1) then
        close(13); close(17); close(18)
        open(unit=16, file=trim(out_dir)//'/dd_pass1.bin', access='stream', form='unformatted', status='replace')
        write(16) dd
        close(16)
     endif
     if (ndec .eq. 0) exit
  enddo

  close(15)

contains

  subroutine compute_savg_linear(dd_in, savg_out)
    include 'ft4_params.f90'
    real, intent(in) :: dd_in(NMAX)
    real, intent(out) :: savg_out(NH1)
    real :: xbuf(NFFT1), winbuf(NFFT1)
    complex :: cxbuf(0:NH1)
    equivalence (xbuf, cxbuf)
    integer :: jj, iaa, ibb
    winbuf = 0.0
    call nuttal_window(winbuf, NFFT1)
    winbuf = winbuf / 300.0
    savg_out = 0.0
    do jj = 1, NHSYM
       iaa = (jj - 1) * NSTEP + 1
       ibb = iaa + NFFT1 - 1
       xbuf = dd_in(iaa:ibb) * winbuf
       call four2a(xbuf, NFFT1, 1, -1, 0)
       savg_out = savg_out + abs(cxbuf(1:NH1))**2
    enddo
    savg_out = savg_out / real(NHSYM)
  end subroutine compute_savg_linear

  subroutine build_cq_prefix(msg_in, out_bits, rvec_in)
    use packjt77
    character(len=*), intent(in) :: msg_in
    integer*1, intent(out) :: out_bits(29)
    integer, intent(in) :: rvec_in(77)
    character(len=37) :: m37
    character(len=77) :: bits77
    integer :: i3t, n3t, ii, bb
    m37 = msg_in
    i3t = -1; n3t = -1
    call pack77(m37, i3t, n3t, bits77)
    do ii = 1, 29
       read(bits77(ii:ii), '(I1)') bb
       out_bits(ii) = int(ieor(bb, rvec_in(ii)), 1)
    enddo
  end subroutine build_cq_prefix

  subroutine apply_cq_llr(nct, amag, q0, qt, qf, qru, qww, lvec)
    integer, intent(in) :: nct
    real, intent(in) :: amag
    integer*1, intent(in) :: q0(29), qt(29), qf(29), qru(29), qww(29)
    real, intent(inout) :: lvec(174)
    integer*1 :: sel(29)
    if (nct .eq. 1 .or. nct .eq. 2) then
       sel = qt
    elseif (nct .eq. 3) then
       sel = qf
    elseif (nct .eq. 4) then
       sel = qru
    elseif (nct .eq. 5) then
       sel = qww
    else
       sel = q0
    endif
    lvec(1:29) = amag * (2.0 * real(sel) - 1.0)
  end subroutine apply_cq_llr

  subroutine count_sync_qual_ft4(bmet, qual_out)
    real, intent(in) :: bmet(206, 3)
    integer, intent(out) :: qual_out
    integer :: sb(8), sa(8), sc(8), sd(8), ib(8), ii
    data sa/0,0,0,1,1,0,1,1/
    data sb/0,1,0,0,1,1,1,0/
    data sc/1,1,1,0,0,1,0,0/
    data sd/1,0,1,1,0,0,0,1/
    qual_out = 0
    do ii = 1, 8
       if (bmet(ii, 1) .ge. 0.0 .eqv. (sa(ii) .eq. 1)) qual_out = qual_out + 1
       if (bmet(66+ii, 1) .ge. 0.0 .eqv. (sb(ii) .eq. 1)) qual_out = qual_out + 1
       if (bmet(132+ii, 1) .ge. 0.0 .eqv. (sc(ii) .eq. 1)) qual_out = qual_out + 1
       if (bmet(198+ii, 1) .ge. 0.0 .eqv. (sd(ii) .eq. 1)) qual_out = qual_out + 1
    enddo
  end subroutine count_sync_qual_ft4

  subroutine extract_ft4_payload_llrs(bmet, la, lb, lc)
    real, intent(in) :: bmet(206, 3)
    real, intent(out) :: la(174), lb(174), lc(174)
    la(1:58) = SCALEFAC * bmet(9:66, 1)
    la(59:116) = SCALEFAC * bmet(75:132, 1)
    la(117:174) = SCALEFAC * bmet(141:198, 1)
    lb(1:58) = SCALEFAC * bmet(9:66, 2)
    lb(59:116) = SCALEFAC * bmet(75:132, 2)
    lb(117:174) = SCALEFAC * bmet(141:198, 2)
    lc(1:58) = SCALEFAC * bmet(9:66, 3)
    lc(59:116) = SCALEFAC * bmet(75:132, 3)
    lc(117:174) = SCALEFAC * bmet(141:198, 3)
  end subroutine extract_ft4_payload_llrs

end program ft4_stage4_dump
